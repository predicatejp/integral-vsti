// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 発振器回路の定常入力と内部状態を分離した単一のオシレータ。

use crate::envelope::{self, EnvelopeParameters};

/// 対称な三角波・ノコギリ波の実効値をピーク振幅へ変換する倍率（√3）。
const SQRT_3: f64 = 1.732_050_807_568_877_2;

/// 位相から生成する波形。矩形波はデューティ比50%、ノコギリ波は右上がり。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    /// 位相0から正方向に立ち上がるサイン波。
    Sine,
    /// 位相0で0、1/4で正のピーク、3/4で負のピークとなる三角波。
    Triangle,
    /// 前半周期が正、後半周期が負の50%デューティ矩形波。
    Square,
    /// 位相0の負のピークから直線的に上昇し、周期境界で折り返すノコギリ波。
    Sawtooth,
}

impl Waveform {
    /// 理想波形の1周期における実効値をピーク振幅へ変換する倍率。
    fn peak_per_rms(self) -> f64 {
        match self {
            Self::Sine => std::f64::consts::SQRT_2,
            Self::Triangle | Self::Sawtooth => SQRT_3,
            Self::Square => 1.0,
        }
    }

    /// 位相[0, 1)からピーク振幅1の波形を生成する。帯域制限は行わない。
    fn unit_sample(self, phase: f64) -> f64 {
        match self {
            Self::Sine => (std::f64::consts::TAU * phase).sin(),
            Self::Triangle => 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs(),
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Sawtooth => 2.0 * phase - 1.0,
        }
    }
}

/// イベントではなく、サンプルごとに参照する発振器回路の定常入力。
/// 周波数・振幅は目標値。現在の音量と時間はOscillatorStateが保持する。
#[derive(Clone, Copy)]
pub struct OscillatorInput {
    /// 生成する波形。変更しても内部の位相を初期化しない。
    pub waveform: Waveform,
    /// 同じノートを鳴らすユニゾンのボイス数（1..=7）。
    pub unison_voices: u32,
    /// 中心音程から両端までの幅（0..=100 cent）。1ボイス時には適用しない。
    pub detune_cents: f64,
    /// 目標周波数（Hz）。有限の0以上の値。入力を変更しても位相は初期化しない。
    pub frequency_hz: f64,
    /// 理想波形の目標実効値（0..=1）。ピーク値は波形に依存する。0でもgateがtrueなら位相は進む。
    pub amplitude: f64,
    /// 発音を指示する定常ゲート。falseでreleaseへ移り、完了後に位相を止める。
    pub gate: bool,
    /// 音量の時間変化。sustainは目標実効値に対する倍率。
    pub envelope: EnvelopeParameters,
    /// Note Onごとに増える世代番号。同音高・ゲート開のままでも再attackを検出する。
    pub note_generation: u64,
    /// 処理停止ごとに増える番号。次サンプルでreleaseを経ずに音量を消去する。
    pub reset_generation: u64,
}

impl OscillatorInput {
    fn rendered_voices(&self) -> usize {
        if self.detune_cents == 0.0 {
            1
        } else {
            self.unison_voices as usize
        }
    }

    fn frequency_ratio(&self, k: usize) -> f64 {
        if self.unison_voices == 1 || self.detune_cents == 0.0 {
            return 1.0;
        }
        let cents = self.detune_cents * (2.0 * k as f64 / f64::from(self.unison_voices - 1) - 1.0);
        2.0_f64.powf(cents / 1200.0)
    }

    pub fn is_valid(&self) -> bool {
        self.frequency_hz.is_finite()
            && self.frequency_hz >= 0.0
            && (0.0..=1.0).contains(&self.amplitude)
            && (1..=7).contains(&self.unison_voices)
            && (0.0..=100.0).contains(&self.detune_cents)
            && self.envelope.is_valid()
    }
}

/// 発振器回路の内部状態。イベント処理からは書き換えない。
#[derive(Clone, Copy)]
pub struct OscillatorState {
    /// 次に生成するサンプルの位相（周期単位、0以上1未満）。
    /// 初期値は0。入力変更や再発音でも保持し、release完了後と処理停止中は進めない。
    /// 未使用ボイスも位相を保持し、再使用時に引き継ぐ。
    phases: [f64; 7],
    /// 次サンプル時点の実効振幅。再Note Onの開始音量として引き継ぐ。
    current_amplitude: f64,
    /// 直近のNote On時の実効振幅。次のNote Onまで固定する。
    start_amplitude: f64,
    /// 直近のNote Onから次サンプルまでのサンプル数。累積加算による時刻誤差を避ける。
    elapsed_samples: u64,
    /// ノートオンからノートオフまでのサンプル数。未到達はNone。
    note_off_sample: Option<u64>,
    /// releaseを含め発振器の時間と位相を進めている間はtrue。
    active: bool,
    /// サンプル処理へ反映済みのNote On世代番号。
    note_generation: u64,
    /// サンプル処理へ反映済みの即時停止番号。
    reset_generation: u64,
    /// 周波数倍率・合成音量の計算に使った（ユニゾン数, Detune）。Noneは未計算。
    unison_input: Option<(u32, f64)>,
    /// 位相増分の計算に使った（基音Hz, サンプルレート）。
    frequency_input: Option<(f64, f64)>,
    /// 各ユニゾンの基音に対する周波数倍率。入力の音程幅が変わったときだけ更新する。
    frequency_ratios: [f64; 7],
    /// 各ユニゾンの1サンプル当たりの位相増分。周波数・レート変更時も更新する。
    phase_increments: [f64; 7],
    /// 合成波形を割る√ボイス数。Detune=0では1。
    unison_divisor: f64,
}

/// Oscillatorの処理を担当し、専用のStateを所有するコンポーネント。
pub struct Oscillator {
    /// このコンポーネントの内部状態。
    state: OscillatorState,
}

impl Oscillator {
    pub fn new() -> Self {
        Self {
            state: OscillatorState {
                phases: [0.0; 7],
                current_amplitude: 0.0,
                start_amplitude: 0.0,
                elapsed_samples: 0,
                note_off_sample: None,
                active: false,
                note_generation: 0,
                reset_generation: 0,
                unison_input: None,
                frequency_input: None,
                frequency_ratios: [1.0; 7],
                phase_increments: [0.0; 7],
                unison_divisor: 1.0,
            },
        }
    }

    /// 現在の入力・状態でサンプルを生成し、その後で次のサンプルに向けて状態を更新する。
    /// sample_rateは処理環境のサンプルレート（Hz）。不正な入力・レートでは無音を返し、状態を保持する。
    pub fn generate_sample(&mut self, input: &OscillatorInput, sample_rate: f64) -> f64 {
        if !input.is_valid() || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return 0.0;
        }
        // ノート境界を反映し、保持した位相で生成した後に次サンプルへ進める。
        // キャッシュを含むState全体のサンプルごとのコピーは行わない。
        let reset = input.reset_generation != self.state.reset_generation;
        if reset {
            self.state.active = false;
            self.state.current_amplitude = 0.0;
        }
        if input.note_generation != self.state.note_generation && (input.gate || !reset) {
            self.state.start_amplitude = self.state.current_amplitude;
            self.state.elapsed_samples = 0;
            self.state.note_off_sample = None;
            self.state.active = true;
        }
        self.state.note_generation = input.note_generation;
        self.state.reset_generation = input.reset_generation;
        if !input.gate && self.state.active && self.state.note_off_sample.is_none() {
            self.state.note_off_sample = Some(self.state.elapsed_samples);
        }
        self.refresh_amplitude(input, sample_rate);
        // 世代更新とrelease完了を反映してから判定する。振幅0のattack中は省略しない。
        if !self.state.active {
            return 0.0;
        }
        self.refresh_frequency(input, sample_rate);
        let sample = self.generate_waveform_sample(input);
        self.update_sample(input, sample_rate);
        sample
    }

    /// 現在の状態を読み取るだけで、位相を進めずにサンプルを生成する。
    fn generate_waveform_sample(&self, input: &OscillatorInput) -> f64 {
        // Detune=0は保持した先頭位相から1ボイス相当で生成し、同相加算を避ける。
        let count = input.rendered_voices();
        let sum: f64 = self.state.phases[..count]
            .iter()
            .map(|&phase| input.waveform.unit_sample(phase))
            .sum();
        self.state.current_amplitude * input.waveform.peak_per_rms() * sum
            / self.state.unison_divisor
    }

    /// 定常入力が変わったサンプルでだけ周波数倍率・位相増分を再計算する。
    fn refresh_frequency(&mut self, input: &OscillatorInput, sample_rate: f64) {
        let unison = (input.unison_voices, input.detune_cents);
        let changed = self.state.unison_input != Some(unison);
        let count = input.rendered_voices();
        if changed {
            for (k, ratio) in self.state.frequency_ratios[..count].iter_mut().enumerate() {
                *ratio = input.frequency_ratio(k);
            }
            self.state.unison_divisor = (count as f64).sqrt();
            self.state.unison_input = Some(unison);
        }
        let frequency = (input.frequency_hz, sample_rate);
        if changed || self.state.frequency_input != Some(frequency) {
            for (increment, ratio) in self.state.phase_increments[..count]
                .iter_mut()
                .zip(&self.state.frequency_ratios[..count])
            {
                let hz = (input.frequency_hz * ratio).min(f64::MAX);
                // 極端に小さなレートでも除算をオーバーフローさせない。
                *increment = (hz % sample_rate) / sample_rate;
            }
            self.state.frequency_input = Some(frequency);
        }
    }

    /// サンプル生成後に、定常入力を用いて内部状態を1サンプル分進める。
    fn update_sample(&mut self, input: &OscillatorInput, sample_rate: f64) {
        if self.state.active {
            for (phase, increment) in self.state.phases[..input.rendered_voices()]
                .iter_mut()
                .zip(&self.state.phase_increments)
            {
                *phase = (*phase + increment).fract();
            }
            self.state.elapsed_samples = self.state.elapsed_samples.saturating_add(1);
            self.refresh_amplitude(input, sample_rate);
        }
    }

    /// 現在時刻の実効振幅を求め、release完了後は位相・時間を止める。
    fn refresh_amplitude(&mut self, input: &OscillatorInput, sample_rate: f64) {
        if self.state.note_off_sample.is_some_and(|off| {
            (self.state.elapsed_samples - off) as f64
                >= (input.envelope.release_ms * sample_rate / 1000.0).ceil()
        }) {
            self.state.active = false;
        }
        self.state.current_amplitude = if self.state.active {
            envelope::amplitude(
                &input.envelope,
                self.state.elapsed_samples as f64 / sample_rate * 1000.0,
                self.state
                    .note_off_sample
                    .map(|off| off as f64 / sample_rate * 1000.0),
                self.state.start_amplitude,
                input.amplitude,
            )
        } else {
            0.0
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::DEFAULT_ENVELOPE;

    fn test_input() -> OscillatorInput {
        OscillatorInput {
            waveform: Waveform::Sine,
            unison_voices: 1,
            detune_cents: 0.0,
            frequency_hz: 1.0,
            amplitude: 0.5,
            gate: true,
            envelope: EnvelopeParameters {
                sustain: 1.0,
                ..DEFAULT_ENVELOPE
            },
            note_generation: 1,
            reset_generation: 0,
        }
    }

    // 波形自体のテストではattack/decayが終了した状態から始める。
    fn sustained() -> Oscillator {
        Oscillator {
            state: OscillatorState {
                active: true,
                elapsed_samples: 10000,
                note_generation: 1,
                ..Oscillator::new().state
            },
        }
    }

    #[test]
    fn inactive_samples_preserve_phase_but_new_notes_and_silent_attack_advance() {
        let mut oscillator = Oscillator::new();
        let mut input = test_input();
        input.frequency_hz = 100.0;
        input.gate = false;
        input.note_generation = 0;
        for _ in 0..16 {
            assert_eq!(oscillator.generate_sample(&input, 1000.0), 0.0);
        }
        assert_eq!(oscillator.state.elapsed_samples, 0);
        input.gate = true;
        input.note_generation += 1;
        assert_eq!(oscillator.generate_sample(&input, 1000.0), 0.0);
        assert_eq!(oscillator.state.elapsed_samples, 1);
        assert_eq!(oscillator.state.phases[0], 0.1);
        assert!(oscillator.generate_sample(&input, 1000.0) > 0.0);
        input.gate = false;
        input.envelope.release_ms = 0.0;
        assert_eq!(oscillator.generate_sample(&input, 1000.0), 0.0);
        let phase = oscillator.state.phases;
        for _ in 0..16 {
            assert_eq!(oscillator.generate_sample(&input, 1000.0), 0.0);
        }
        assert_eq!(oscillator.state.phases, phase);
        input.gate = true;
        input.note_generation += 1;
        oscillator.generate_sample(&input, 1000.0);
        assert_ne!(oscillator.state.phases, phase);
        input.gate = false;
        input.reset_generation += 1;
        assert_eq!(oscillator.generate_sample(&input, 1000.0), 0.0);
        assert_eq!(oscillator.state.reset_generation, input.reset_generation);
    }

    #[test]
    fn cached_increments_follow_samplewise_input_changes_without_resetting_phase() {
        let mut oscillator = sustained();
        let mut input = test_input();
        let mut phases = [0.0_f64; 7];
        for i in 0..128 {
            input.unison_voices = 1 + (i % 7);
            input.detune_cents = if i % 5 == 0 {
                0.0
            } else {
                f64::from(i) * 0.735
            };
            input.frequency_hz = 220.0 + f64::from(i % 11) * 31.0;
            let rate = if i % 3 == 0 { 96_000.0 } else { 48_000.0 };
            oscillator.generate_sample(&input, rate);
            for (k, phase) in phases.iter_mut().enumerate().take(input.rendered_voices()) {
                let cents = if input.unison_voices == 1 {
                    0.0
                } else {
                    input.detune_cents * (2.0 * k as f64 / f64::from(input.unison_voices - 1) - 1.0)
                };
                *phase =
                    (*phase + input.frequency_hz * 2.0_f64.powf(cents / 1200.0) / rate).fract();
            }
            for (actual, expected) in oscillator.state.phases.iter().zip(phases) {
                assert!((actual - expected).abs() < 1e-14);
            }
        }
    }

    #[test]
    fn waveforms_have_expected_shape_and_peak() {
        let cases = [
            (
                Waveform::Sine,
                std::f64::consts::FRAC_1_SQRT_2,
                [
                    0.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    1.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    0.0,
                    -std::f64::consts::FRAC_1_SQRT_2,
                    -1.0,
                    -std::f64::consts::FRAC_1_SQRT_2,
                ],
            ),
            (
                Waveform::Triangle,
                0.866_025_403_784_438_6,
                [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5],
            ),
            (
                Waveform::Square,
                0.5,
                [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0],
            ),
            (
                Waveform::Sawtooth,
                0.866_025_403_784_438_6,
                [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75],
            ),
        ];
        for (waveform, peak, expected) in cases {
            let mut state = sustained();
            let input = OscillatorInput {
                waveform,
                frequency_hz: 1.0,
                amplitude: 0.5,
                gate: true,
                ..test_input()
            };
            for _ in 0..2 {
                for unit in expected {
                    assert!(
                        (state.generate_sample(&input, 8.0) - peak * unit).abs() < 1e-14,
                        "{waveform:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn waveform_changes_and_gate_preserve_phase() {
        let mut state = sustained();
        let mut input = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: 1.0,
            amplitude: 0.5,
            gate: true,
            ..test_input()
        };
        assert_eq!(state.generate_sample(&input, 4.0), 0.0);
        input.waveform = Waveform::Triangle;
        assert!((state.generate_sample(&input, 4.0) - 0.866_025_403_784_438_6).abs() < 1e-14);
        input.waveform = Waveform::Square;
        assert_eq!(state.generate_sample(&input, 4.0), -0.5);
        input.gate = false;
        input.reset_generation += 1;
        input.waveform = Waveform::Sawtooth;
        assert_eq!(state.generate_sample(&input, 4.0), 0.0);
        input.gate = true;
        // 波形/入力だけの検証は保持した位相のままsustainへ復帰させる。
        state.state.active = true;
        state.state.elapsed_samples = 10000;
        state.state.note_off_sample = None;
        assert!((state.generate_sample(&input, 4.0) - 0.433_012_701_892_219_3).abs() < 1e-14);
    }

    #[test]
    fn steady_input_changes_preserve_state_and_apply_to_the_current_sample() {
        let mut state = sustained();
        let mut input = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: 1.0,
            amplitude: 0.5 / std::f64::consts::SQRT_2,
            gate: true,
            ..test_input()
        };
        assert_eq!(state.generate_sample(&input, 4.0), 0.0); // 更新前の位相0で生成
        assert!((state.generate_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        // 入力だけを変更する。保持していた位相1/2から、新しい振幅・周波数で処理する。
        input.frequency_hz = 2.0;
        input.amplitude = 0.25 / std::f64::consts::SQRT_2;
        assert!(state.generate_sample(&input, 8.0).abs() < 1e-14);
        input.gate = false;
        input.reset_generation += 1;
        for _ in 0..7 {
            assert_eq!(state.generate_sample(&input, 8.0), 0.0);
        }
        input.gate = true;
        // 波形/入力だけの検証は保持した位相のままsustainへ復帰させる。
        state.state.active = true;
        state.state.elapsed_samples = 10000;
        state.state.note_off_sample = None;
        assert!((state.generate_sample(&input, 8.0) + 0.25).abs() < 1e-14);
        // 振幅0はゲート閉とは異なり、状態更新を続ける。
        input.amplitude = 0.0;
        assert_eq!(state.generate_sample(&input, 8.0), 0.0);
        input.amplitude = 0.5 / std::f64::consts::SQRT_2;
        assert!((state.generate_sample(&input, 8.0) - 0.5).abs() < 1e-14);
    }

    #[test]
    fn unison_spreads_pitch_and_preserves_each_phase() {
        for voices in 1..=7 {
            let mut input = OscillatorInput {
                unison_voices: voices,
                detune_cents: 100.0,
                frequency_hz: 440.0,
                ..test_input()
            };
            let mut state = sustained();
            let mut expected_phases = [0.0_f64; 7];
            for sample in 0..32 {
                if sample == 16 {
                    input.frequency_hz = 660.0;
                    input.note_generation += 1;
                }
                // 新しいノートでも全位相を引き継ぎ、生成後に指定の周波数分だけ進む。
                state.generate_sample(&input, 48000.0);
                for (k, phase) in expected_phases.iter_mut().enumerate().take(voices as usize) {
                    let cents = if voices == 1 {
                        0.0
                    } else {
                        -100.0 + 200.0 * k as f64 / f64::from(voices - 1)
                    };
                    *phase = (*phase + input.frequency_hz * 2.0_f64.powf(cents / 1200.0) / 48000.0)
                        .fract();
                    assert!((state.state.phases[k] - *phase).abs() < 1e-12);
                }
            }
            let held_phases = state.state.phases;
            input.gate = false;
            input.reset_generation += 1;
            state.generate_sample(&input, 48000.0);
            assert_eq!(state.state.phases, held_phases);
            // 0 centに切り替えると、過去に異なる位相へ進んでいても先頭だけで生成。
            input.detune_cents = 0.0;
            input.gate = true;
            input.note_generation += 1;
            input.envelope.attack_ms = 0.0;
            let expected =
                0.5 * std::f64::consts::SQRT_2 * (std::f64::consts::TAU * expected_phases[0]).sin();
            assert!((state.generate_sample(&input, 48000.0) - expected).abs() < 1e-12);
            assert_eq!(&state.state.phases[1..], &held_phases[1..]);
        }
    }

    #[test]
    #[cfg(not(miri))] // 長時間平均の数値検証は通常のRust実行で行う。
    fn unison_waveforms_have_target_long_term_rms() {
        for waveform in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for voices in 1..=7 {
                for detune in [0.0, 100.0] {
                    let input = OscillatorInput {
                        waveform,
                        unison_voices: voices,
                        detune_cents: detune,
                        frequency_hz: 440.0,
                        ..test_input()
                    };
                    let mut state = sustained();
                    let mut energy = 0.0;
                    for _ in 0..48000 {
                        let sample = state.generate_sample(&input, 24000.0);
                        assert!(sample.is_finite());
                        energy += sample * sample;
                    }
                    let rms = (energy / 48000.0).sqrt();
                    assert!(
                        (rms - 0.5).abs() < 0.01,
                        "{waveform:?}: N={voices}, detune={detune}, rms={rms}"
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_inputs_preserve_state_and_never_emit_nonfinite_samples() {
        let mut state = sustained();
        let input = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: 1.0,
            amplitude: 0.5 / std::f64::consts::SQRT_2,
            gate: true,
            ..test_input()
        };
        state.generate_sample(&input, 4.0); // 位相1/4を保持しておく。
        for frequency_hz in [f64::NAN, f64::INFINITY, -1.0] {
            let invalid = OscillatorInput {
                frequency_hz,
                ..input
            };
            assert!(!invalid.is_valid());
            assert_eq!(state.generate_sample(&invalid, 4.0), 0.0);
        }
        for amplitude in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            let invalid = OscillatorInput { amplitude, ..input };
            assert!(!invalid.is_valid());
            assert_eq!(state.generate_sample(&invalid, 4.0), 0.0);
        }
        for unison_voices in [0, 8, u32::MAX] {
            assert_eq!(
                state.generate_sample(
                    &OscillatorInput {
                        unison_voices,
                        ..input
                    },
                    4.0
                ),
                0.0
            );
        }
        for detune_cents in [-1.0, 101.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                state.generate_sample(
                    &OscillatorInput {
                        detune_cents,
                        ..input
                    },
                    4.0
                ),
                0.0
            );
        }
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(state.generate_sample(&input, rate), 0.0);
        }
        assert!((state.generate_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        let extreme = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: f64::MAX,
            amplitude: 1.0,
            gate: true,
            ..test_input()
        };
        for rate in [f64::MIN_POSITIVE, 44100.0, 96000.0] {
            for _ in 0..16 {
                assert!(state.generate_sample(&extreme, rate).is_finite());
            }
        }
    }
}

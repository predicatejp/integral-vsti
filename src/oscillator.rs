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
    pub fn is_valid(&self) -> bool {
        self.frequency_hz.is_finite()
            && self.frequency_hz >= 0.0
            && (0.0..=1.0).contains(&self.amplitude)
            && self.envelope.is_valid()
    }
}

/// 発振器回路の内部状態。イベント処理からは書き換えない。
#[derive(Clone, Copy)]
pub struct OscillatorState {
    /// 次に生成するサンプルの位相（周期単位、0以上1未満）。
    /// 初期値は0。入力変更や再発音でも保持し、release完了後と処理停止中は進めない。
    phase: f64,
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
}

impl OscillatorState {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            current_amplitude: 0.0,
            start_amplitude: 0.0,
            elapsed_samples: 0,
            note_off_sample: None,
            active: false,
            note_generation: 0,
            reset_generation: 0,
        }
    }

    /// 現在の入力・状態でサンプルを生成し、その後で次のサンプルに向けて状態を更新する。
    /// sample_rateは処理環境のサンプルレート（Hz）。不正な入力・レートでは無音を返し、状態を保持する。
    pub fn next_sample(&mut self, input: &OscillatorInput, sample_rate: f64) -> f64 {
        if !input.is_valid() || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return 0.0;
        }
        // 入力を反映した作業値で生成し、内部状態の書き戻しは生成・更新後に行う。
        let mut next = *self;
        if input.reset_generation != self.reset_generation {
            next.active = false;
            next.current_amplitude = 0.0;
        }
        if input.note_generation != self.note_generation
            && (input.gate || input.reset_generation == self.reset_generation)
        {
            next.start_amplitude = next.current_amplitude;
            next.elapsed_samples = 0;
            next.note_off_sample = None;
            next.active = true;
        }
        next.note_generation = input.note_generation;
        next.reset_generation = input.reset_generation;
        if !input.gate && next.active && next.note_off_sample.is_none() {
            next.note_off_sample = Some(next.elapsed_samples);
        }
        next.refresh_amplitude(input, sample_rate);
        let sample = next.generate_sample(input);
        next.update(input, sample_rate);
        *self = next;
        sample
    }

    /// 現在の状態を読み取るだけで、位相を進めずにサンプルを生成する。
    fn generate_sample(&self, input: &OscillatorInput) -> f64 {
        self.current_amplitude
            * input.waveform.peak_per_rms()
            * input.waveform.unit_sample(self.phase)
    }

    /// サンプル生成後に、定常入力を用いて内部状態を1サンプル分進める。
    fn update(&mut self, input: &OscillatorInput, sample_rate: f64) {
        if self.active {
            // 小さなサンプルレートでも除算のオーバーフローを起こさない。
            let increment = (input.frequency_hz % sample_rate) / sample_rate;
            self.phase = (self.phase + increment).fract();
            self.elapsed_samples = self.elapsed_samples.saturating_add(1);
            self.refresh_amplitude(input, sample_rate);
        }
    }

    /// 現在時刻の実効振幅を求め、release完了後は位相・時間を止める。
    fn refresh_amplitude(&mut self, input: &OscillatorInput, sample_rate: f64) {
        if self.note_off_sample.is_some_and(|off| {
            (self.elapsed_samples - off) as f64
                >= (input.envelope.release_ms * sample_rate / 1000.0).ceil()
        }) {
            self.active = false;
        }
        self.current_amplitude = if self.active {
            envelope::amplitude(
                &input.envelope,
                self.elapsed_samples as f64 / sample_rate * 1000.0,
                self.note_off_sample
                    .map(|off| off as f64 / sample_rate * 1000.0),
                self.start_amplitude,
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
    fn sustained() -> OscillatorState {
        OscillatorState {
            active: true,
            elapsed_samples: 10000,
            note_generation: 1,
            ..OscillatorState::new()
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
                        (state.next_sample(&input, 8.0) - peak * unit).abs() < 1e-14,
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
        assert_eq!(state.next_sample(&input, 4.0), 0.0);
        input.waveform = Waveform::Triangle;
        assert!((state.next_sample(&input, 4.0) - 0.866_025_403_784_438_6).abs() < 1e-14);
        input.waveform = Waveform::Square;
        assert_eq!(state.next_sample(&input, 4.0), -0.5);
        input.gate = false;
        input.reset_generation += 1;
        input.waveform = Waveform::Sawtooth;
        assert_eq!(state.next_sample(&input, 4.0), 0.0);
        input.gate = true;
        // 波形/入力だけの検証は保持した位相のままsustainへ復帰させる。
        state.active = true;
        state.elapsed_samples = 10000;
        state.note_off_sample = None;
        assert!((state.next_sample(&input, 4.0) - 0.433_012_701_892_219_3).abs() < 1e-14);
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
        assert_eq!(state.next_sample(&input, 4.0), 0.0); // 更新前の位相0で生成
        assert!((state.next_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        // 入力だけを変更する。保持していた位相1/2から、新しい振幅・周波数で処理する。
        input.frequency_hz = 2.0;
        input.amplitude = 0.25 / std::f64::consts::SQRT_2;
        assert!(state.next_sample(&input, 8.0).abs() < 1e-14);
        input.gate = false;
        input.reset_generation += 1;
        for _ in 0..7 {
            assert_eq!(state.next_sample(&input, 8.0), 0.0);
        }
        input.gate = true;
        // 波形/入力だけの検証は保持した位相のままsustainへ復帰させる。
        state.active = true;
        state.elapsed_samples = 10000;
        state.note_off_sample = None;
        assert!((state.next_sample(&input, 8.0) + 0.25).abs() < 1e-14);
        // 振幅0はゲート閉とは異なり、状態更新を続ける。
        input.amplitude = 0.0;
        assert_eq!(state.next_sample(&input, 8.0), 0.0);
        input.amplitude = 0.5 / std::f64::consts::SQRT_2;
        assert!((state.next_sample(&input, 8.0) - 0.5).abs() < 1e-14);
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
        state.next_sample(&input, 4.0); // 位相1/4を保持しておく。
        for frequency_hz in [f64::NAN, f64::INFINITY, -1.0] {
            let invalid = OscillatorInput {
                frequency_hz,
                ..input
            };
            assert!(!invalid.is_valid());
            assert_eq!(state.next_sample(&invalid, 4.0), 0.0);
        }
        for amplitude in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            let invalid = OscillatorInput { amplitude, ..input };
            assert!(!invalid.is_valid());
            assert_eq!(state.next_sample(&invalid, 4.0), 0.0);
        }
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(state.next_sample(&input, rate), 0.0);
        }
        assert!((state.next_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        let extreme = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: f64::MAX,
            amplitude: 1.0,
            gate: true,
            ..test_input()
        };
        for rate in [f64::MIN_POSITIVE, 44100.0, 96000.0] {
            for _ in 0..16 {
                assert!(state.next_sample(&extreme, rate).is_finite());
            }
        }
    }
}

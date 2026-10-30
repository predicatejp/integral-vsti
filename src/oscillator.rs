// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 発振器回路の定常入力と内部状態を分離した単一のオシレータ。

use crate::envelope::{Envelope, EnvelopeParameters};
use std::{collections::hash_map::RandomState, hash::BuildHasher};

#[path = "oscillator_tables.rs"]
mod tables;

/// 等間隔テーブルの隣接点を線形補間する。
/// SAFETY: positionは有限の0〜1024。tableは2〜1025要素で、positionは最終位置以下。
unsafe fn interpolate(position: f64, table: &[f64]) -> f64 {
    debug_assert!((2..=1025).contains(&table.len()));
    debug_assert!((0.0..=(table.len() - 1) as f64).contains(&position));
    // SAFETY: 呼び出し元が保証する有限の0〜1024は、小数部を切り捨てるとi32に収まる。
    let index = unsafe { position.to_int_unchecked::<i32>() } as u32;
    let index = index.min((table.len() - 2) as u32);
    let at = index as usize;
    table[at] + (table[at + 1] - table[at]) * (position - f64::from(index))
}

/// ユニゾン7音分の左右倍率。Synthesizerが1組を保持し、各Voiceへ借用で渡す。
pub type PanningGains = [[f64; 2]; 7];

/// 実発音数・Width・Panから共通の定位係数を作る純粋計算。不正値には無音係数を返す。
/// 中央・端点以外は既存の四半周期表でequal-powerのsin/cosを線形補間する。
pub fn panning_gains(count: usize, width: f64, pan: f64) -> PanningGains {
    if !((1..=7).contains(&count) & (0.0..=1.0).contains(&width) & (-1.0..=1.0).contains(&pan)) {
        return [[0.0; 2]; 7];
    }
    let mut result = [[1.0; 2]; 7];
    for (k, gains) in result[..count].iter_mut().enumerate() {
        let spread = if count == 1 {
            0.0
        } else {
            2.0 * k as f64 / (count - 1) as f64 - 1.0
        };
        let position = (pan + (1.0 - pan.abs()) * width * spread).clamp(-1.0, 1.0);
        *gains = match position {
            -1.0 => [std::f64::consts::SQRT_2, 0.0],
            0.0 => [1.0; 2],
            1.0 => [0.0, std::f64::consts::SQRT_2],
            _ => {
                let x = (position + 1.0) * 512.0;
                // SAFETY: 有限のpositionを[-1,1]へ制限済み。両位置は0〜1024、表は1025要素。
                let (left, right) = unsafe {
                    (
                        interpolate(1024.0 - x, &tables::SINE),
                        interpolate(x, &tables::SINE),
                    )
                };
                [
                    left * std::f64::consts::SQRT_2,
                    right * std::f64::consts::SQRT_2,
                ]
            }
        };
    }
    result
}

/// −100〜100 centの倍率。ホストの連続値やユニゾン配置の小数centも保持する。
fn detune_ratio(cents: f64) -> f64 {
    // SAFETY: 検証済みのDetuneとユニゾン配置からcentsは有限の−100〜100。表は201要素。
    unsafe { interpolate(cents + 100.0, &tables::DETUNE) }
}

/// 1/4周期1024区間と対称性による正弦波。入力位相は[0, 1)。実効値補正は行わない。
fn sine_sample(phase: f64) -> f64 {
    let position = phase * 4.0;
    let quadrant = position as u32;
    let fraction = position - f64::from(quadrant);
    // 偶数象限はfraction、奇数象限は1-fractionへ折り返す。
    let folded = (f64::from(quadrant & 1) - fraction).abs();
    // SAFETY: 位相は有限の0〜1（更新時の丸めによる1も含む）。折り返し位置は0〜1024。
    let value = unsafe { interpolate(folded * 1024.0, &tables::SINE) };
    value * (1.0 - f64::from(quadrant & 2))
}

/// SplitMix64の出力変換。位相生成とノイズ状態の初期化で共用する純粋計算。
fn splitmix64(mut bits: u64) -> u64 {
    bits = (bits ^ (bits >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    bits = (bits ^ (bits >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    bits ^ (bits >> 31)
}

/// 三角波・ノコギリ波・一様ノイズの実効値をピーク振幅へ変換する倍率（√3）。
const SQRT_3: f64 = 1.732_050_807_568_877_2;

/// 周期波4種と一様分布のホワイトノイズ。
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
    /// 毎サンプル別の一様乱数を使うホワイトノイズ。
    Noise,
}

impl Waveform {
    /// 理想波形の実効値をピーク振幅へ変換する倍率。Noiseは分布の二乗平均による。
    fn peak_per_rms(self) -> f64 {
        match self {
            Self::Sine => std::f64::consts::SQRT_2,
            Self::Triangle | Self::Sawtooth | Self::Noise => SQRT_3,
            Self::Square => 1.0,
        }
    }

    /// 周期波の位相またはNoiseの一様乱数[0, 1)からピーク振幅1のサンプルを生成する。
    fn unit_sample(self, phase: f64) -> f64 {
        match self {
            Self::Sine => sine_sample(phase),
            Self::Triangle => 4.0 * ((phase - 0.25).abs() - 0.5).abs() - 1.0,
            // phase=0.5の+0も負側へ含めるため、符号を反転する。
            Self::Square => -1.0_f64.copysign(phase - 0.5),
            Self::Sawtooth | Self::Noise => 2.0 * phase - 1.0,
        }
    }
}

/// Note On境界で各ユニゾン位相へ適用する動作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseMode {
    /// 直前の位相を保持する。
    Keep,
    /// すべての位相を0にする。
    Zero,
    /// 各位相を独立した疑似乱数で[0, 1)へ設定する。
    Random,
}

/// イベントではなく、サンプルごとに参照する発振器回路の定常入力。
/// 周波数・振幅は目標値。現在の音量と時間はOscillatorStateが保持する。
#[derive(Clone, Copy)]
pub struct OscillatorInput {
    /// 生成する波形。変更しても内部の位相を初期化しない。
    pub waveform: Waveform,
    /// 次のNote Onで適用する位相モード。
    pub phase_mode: PhaseMode,
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
        1 + (self.unison_voices as usize - 1) * usize::from(self.detune_cents != 0.0)
    }

    fn frequency_ratio(&self, k: usize) -> f64 {
        let intervals = self.unison_voices - 1;
        // N=1では分母を1、中心からの差を0として、同じ式で倍率1を得る。
        let cents = self.detune_cents
            * (2.0 * k as f64 / f64::from(intervals.max(1)) - f64::from(intervals.min(1)));
        detune_ratio(cents)
    }

    pub fn is_valid(&self) -> bool {
        // 非負の有限値という条件を上下限でまとめ、分類判定を重ねない。
        (self.frequency_hz <= f64::MAX)
            & (self.frequency_hz >= 0.0)
            & (0.0..=1.0).contains(&self.amplitude)
            & (1..=7).contains(&self.unison_voices)
            & (0.0..=100.0).contains(&self.detune_cents)
            & self.envelope.is_valid()
    }
}

/// 発振器回路の内部状態。イベント処理からは書き換えない。
#[derive(Clone, Copy)]
pub struct OscillatorState {
    /// 次に生成するサンプルの位相（周期単位、0以上1未満）。
    /// 初期値は0。Note On時はphase_modeに従い、release完了後と処理停止中は進めない。
    phases: [f64; 7],
    /// Note On時だけ進めるSplitMix64の内部状態。初期化時に発振器ごとの種を取得する。
    random: u64,
    /// ノイズ専用xoshiro256+の状態。各ユニゾンに連続する別の値を割り当てる。
    noise: [u64; 4],
    /// 正規化値のAHDSR。入力の目標実効値をピークとして使用する。
    envelope: Envelope,
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
        let random = RandomState::new().hash_one(0_u64);
        Self {
            state: OscillatorState {
                phases: [0.0; 7],
                random,
                // SplitMix64の連続出力は全要素0にならない。乱数列は位相用と独立に進める。
                noise: std::array::from_fn(|i| {
                    splitmix64(
                        random.wrapping_add(0x9e37_79b9_7f4a_7c15_u64.wrapping_mul(i as u64 + 1)),
                    )
                }),
                envelope: Envelope::new(),
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
    /// gainsはSynthesizerが実発音数・Width・Panから算出した共通の定常入力。
    pub fn generate_sample(
        &mut self,
        input: &OscillatorInput,
        sample_rate: f64,
        gains: &PanningGains,
    ) -> [f64; 2] {
        if !(input.is_valid() & (sample_rate <= f64::MAX) & (sample_rate > 0.0)) {
            return [0.0; 2];
        }
        // ノート境界と位相モードを反映して生成した後、次サンプルへ進める。
        // キャッシュを含むState全体のサンプルごとのコピーは行わない。
        let reset = input.reset_generation != self.state.reset_generation;
        if input.note_generation != self.state.note_generation && (input.gate || !reset) {
            match input.phase_mode {
                PhaseMode::Keep => {}
                PhaseMode::Zero => self.state.phases.fill(0.0),
                PhaseMode::Random => {
                    for k in 0..self.state.phases.len() {
                        self.state.phases[k] = self.random_phase();
                    }
                }
            }
        }
        self.state.note_generation = input.note_generation;
        self.state.reset_generation = input.reset_generation;
        let amplitude = self.state.envelope.generate_sample(
            &input.envelope,
            sample_rate,
            input.gate,
            input.note_generation,
            input.reset_generation,
            input.amplitude,
        );
        // 振幅0のattack中も位相を進め、release完了後は止める。
        if !self.state.envelope.is_active() {
            return [0.0; 2];
        }
        self.refresh_frequency(input, sample_rate);
        let sample = self.generate_waveform_sample(input, amplitude, gains);
        self.update_sample(input, sample_rate);
        sample
    }

    /// SplitMix64の上位53bitを周期単位へ換算。乱数生成はNote On時の整数演算だけで完了する。
    /// 参照元: https://prng.di.unimi.it/splitmix64.c （通知はTHIRD_PARTY_NOTICES.txt）。
    fn random_phase(&mut self) -> f64 {
        self.state.random = self.state.random.wrapping_add(0x9e37_79b9_7f4a_7c15);
        (splitmix64(self.state.random) >> 11) as f64 * (1.0 / (1_u64 << 53) as f64)
    }

    /// xoshiro256+ 1.0の上位53bitを[0,1)へ換算し、次の乱数へ進める。
    /// 参照元: https://prng.di.unimi.it/xoshiro256plus.c （通知はTHIRD_PARTY_NOTICES.txt）。
    fn generate_noise_sample(&mut self) -> f64 {
        let s = &mut self.state.noise;
        let bits = s[0].wrapping_add(s[3]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        (bits >> 11) as f64 * (1.0 / (1_u64 << 53) as f64)
    }

    /// 位相を保持して波形を生成する。Noiseはユニゾンごとに乱数を一つ進める。
    fn generate_waveform_sample(
        &mut self,
        input: &OscillatorInput,
        amplitude: f64,
        gains: &PanningGains,
    ) -> [f64; 2] {
        let count = input.rendered_voices();
        let mut sum = [0.0; 2];
        // 乱数生成の判定はユニゾンの内側で繰り返さない。
        if input.waveform == Waveform::Noise {
            for gains in &gains[..count] {
                let sample = Waveform::Noise.unit_sample(self.generate_noise_sample());
                sum[0] += sample * gains[0];
                sum[1] += sample * gains[1];
            }
        } else {
            for (&phase, gains) in self.state.phases[..count].iter().zip(gains) {
                let sample = input.waveform.unit_sample(phase);
                sum[0] += sample * gains[0];
                sum[1] += sample * gains[1];
            }
        }
        sum.map(|sample| {
            amplitude * input.waveform.peak_per_rms() * sample / self.state.unison_divisor
        })
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
                // 通常音程では剰余を省く。高周波では除算のオーバーフローを防ぐ。
                *increment = if hz < sample_rate {
                    hz
                } else {
                    hz % sample_rate
                } / sample_rate;
            }
            self.state.frequency_input = Some(frequency);
        }
    }

    /// 発音中のサンプル生成後に、定常入力を用いて内部状態を1サンプル分進める。
    fn update_sample(&mut self, input: &OscillatorInput, sample_rate: f64) {
        // generate_sampleが発音中であることを確認してから呼ぶ。
        for (phase, increment) in self.state.phases[..input.rendered_voices()]
            .iter_mut()
            .zip(&self.state.phase_increments)
        {
            // 位相・増分は[0, 1)なので、和は[0, 2)。1周期だけ引けばよい。
            let next = *phase + increment;
            *phase = next - f64::from((next >= 1.0) as u8);
        }
        self.state
            .envelope
            .update_sample(&input.envelope, sample_rate, input.amplitude);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::DEFAULT_ENVELOPE;

    const NOISE_SEED: [u64; 4] = [
        0x1234_5678_9abc_def0,
        0xfedc_ba98_7654_3210,
        0x1020_3040_5060_7080,
        0x8877_6655_4433_2211,
    ];

    #[test]
    fn noise_matches_reference_sequence() {
        let mut oscillator = Oscillator::new();
        oscillator.state.noise = NOISE_SEED;
        // 作者のxoshiro256+ 1.0 C実装を同じ初期状態で実行した上位53bit。
        for expected in [
            5441996726197760_u64,
            2484858452741806,
            5686259996541749,
            7701203122316008,
            8048927005211174,
            5195266920391698,
            2168565655257186,
            1019535850546686,
        ] {
            assert_eq!(
                oscillator.generate_noise_sample(),
                expected as f64 / (1_u64 << 53) as f64
            );
        }
    }

    #[test]
    fn noise_controls_preserve_sequence_and_stop_after_release() {
        let mut input = test_input();
        input.waveform = Waveform::Noise;
        input.envelope.attack_ms = 0.0;
        input.envelope.decay_ms = 0.0;
        input.envelope.release_ms = 1.0;
        let mut initial = sustained();
        initial.state.noise = NOISE_SEED;
        for n in 1..=7 {
            for detune in [0.0, 1.0, 100.0] {
                for mode in [PhaseMode::Keep, PhaseMode::Zero, PhaseMode::Random] {
                    let mut reference = Oscillator {
                        state: initial.state,
                    };
                    let mut actual = Oscillator {
                        state: initial.state,
                    };
                    let a = OscillatorInput {
                        unison_voices: n,
                        detune_cents: detune,
                        phase_mode: mode,
                        frequency_hz: 880.0,
                        ..input
                    };
                    let b = OscillatorInput {
                        unison_voices: if detune == 0.0 { 1 } else { n },
                        detune_cents: if detune == 0.0 { 0.0 } else { 25.0 },
                        frequency_hz: 0.0,
                        ..input
                    };
                    let gains = panning_gains(a.rendered_voices(), 1.0, 0.0);
                    for i in 0..16 {
                        // 再ノートオンでもPhaseに依存せず、ノイズ列を引き継ぐ。
                        let generation = 1 + i / 4;
                        let a = OscillatorInput {
                            note_generation: generation,
                            ..a
                        };
                        let b = OscillatorInput {
                            note_generation: generation,
                            ..b
                        };
                        assert_eq!(
                            actual.generate_sample(&a, 96000.0, &gains),
                            reference.generate_sample(&b, 44100.0, &gains)
                        );
                    }
                }
            }
        }
        let mut oscillator = initial;
        let gains = panning_gains(1, 1.0, 0.0);
        oscillator.generate_sample(&input, 48000.0, &gains);
        input.gate = false;
        for _ in 0..49 {
            oscillator.generate_sample(&input, 48000.0, &gains);
        }
        let noise = oscillator.state.noise;
        assert_eq!(
            oscillator.generate_sample(&input, 48000.0, &gains),
            [0.0; 2]
        );
        assert_eq!(oscillator.state.noise, noise);
        input.gate = true;
        input.note_generation += 1;
        oscillator.generate_sample(&input, 48000.0, &gains);
        assert_ne!(oscillator.state.noise, noise);
        input.reset_generation += 1;
        input.gate = false;
        let noise = oscillator.state.noise;
        assert_eq!(
            oscillator.generate_sample(&input, 48000.0, &gains),
            [0.0; 2]
        );
        assert_eq!(oscillator.state.noise, noise);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 固定シードによる長時間の統計検証は通常のCPUで行う。
    fn noise_rms_mean_and_correlation_follow_unison_panning() {
        for n in [1, 2, 7] {
            for width in [0.0, 1.0] {
                let mut oscillator = sustained();
                oscillator.state.noise = NOISE_SEED;
                let input = OscillatorInput {
                    waveform: Waveform::Noise,
                    unison_voices: n,
                    detune_cents: 25.0,
                    ..test_input()
                };
                let gains = panning_gains(n as usize, width, 0.0);
                let (mut sum, mut energy, mut cross, mut lag) = ([0.0; 2], [0.0; 2], 0.0, 0.0);
                let mut previous = 0.0;
                for _ in 0..65536 {
                    let sample = oscillator.generate_sample(&input, 48000.0, &gains);
                    for ch in 0..2 {
                        assert!(sample[ch].is_finite());
                        sum[ch] += sample[ch];
                        energy[ch] += sample[ch] * sample[ch];
                    }
                    cross += sample[0] * sample[1];
                    lag += sample[0] * previous;
                    previous = sample[0];
                    if width == 0.0 || n == 1 {
                        assert_eq!(sample[0], sample[1]);
                    }
                }
                for ch in 0..2 {
                    assert!((sum[ch] / 65536.0).abs() < 0.01);
                    assert!(((energy[ch] / 65536.0).sqrt() - 0.5).abs() < 0.01);
                }
                assert!((lag / energy[0]).abs() < 0.025);
                let expected =
                    gains[..n as usize].iter().map(|g| g[0] * g[1]).sum::<f64>() / f64::from(n);
                let correlation = cross / (energy[0] * energy[1]).sqrt();
                assert!((correlation - expected).abs() < 0.025);
            }
        }
    }

    #[test]
    fn panning_lookup_bounds_and_exact_endpoints() {
        // 全1024区間の端点と内部7点。線形補間の最大誤差を解析式と比較する。
        for i in 0..=8192 {
            let pan = i as f64 / 4096.0 - 1.0;
            let [left, right] = panning_gains(1, 0.0, pan)[0];
            let angle = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
            assert!((left - std::f64::consts::SQRT_2 * angle.cos()).abs() < 4.17e-7);
            assert!((right - std::f64::consts::SQRT_2 * angle.sin()).abs() < 4.17e-7);
            assert!((left * left + right * right - 2.0).abs() < 1.18e-6);
        }
        for count in 1..=7 {
            for width in [0.0, 0.37, 1.0] {
                assert_eq!(
                    &panning_gains(count, width, -1.0)[..count],
                    &[[std::f64::consts::SQRT_2, 0.0]; 7][..count]
                );
                assert_eq!(
                    &panning_gains(count, width, 1.0)[..count],
                    &[[0.0, std::f64::consts::SQRT_2]; 7][..count]
                );
            }
            assert_eq!(panning_gains(count, 0.0, 0.0), [[1.0; 2]; 7]);
        }
        for (count, width, pan) in [
            (0, 0.0, 0.0),
            (8, 0.0, 0.0),
            (1, f64::NAN, 0.0),
            (1, 1.1, 0.0),
            (1, -0.1, 0.0),
            (1, 0.0, f64::INFINITY),
            (1, 0.0, -1.1),
            (1, 0.0, f64::NAN),
        ] {
            assert_eq!(panning_gains(count, width, pan), [[0.0; 2]; 7]);
        }
    }

    #[test]
    fn stereo_panning_matches_equal_power_and_preserves_phase() {
        let mut input = test_input();
        input.frequency_hz = 440.0;
        input.envelope.attack_ms = 0.0;
        let mut oscillator = Oscillator::new();
        for n in 1..=7 {
            for detune in [0.0, 25.0] {
                for width in [0.0, 0.37, 1.0] {
                    for pan in [-1.0_f64, -0.63, 0.0, 0.28, 1.0] {
                        input.unison_voices = n;
                        input.detune_cents = detune;
                        oscillator.state.phases = [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75];
                        let before = oscillator.state.phases;
                        let count = if detune == 0.0 { 1 } else { n as usize };
                        let mut expected = [0.0; 2];
                        for (k, &phase) in before[..count].iter().enumerate() {
                            let spread = if count == 1 {
                                0.0
                            } else {
                                2.0 * k as f64 / (count - 1) as f64 - 1.0
                            };
                            let position = pan + width * (1.0 - pan.abs()) * spread;
                            let angle = (position + 1.0) * std::f64::consts::FRAC_PI_4;
                            // Sineのピーク0.5*sqrt(2)、中央補正sqrt(2)、ユニゾン補正1/sqrt(N)。
                            let sample = input.waveform.unit_sample(phase) / (count as f64).sqrt();
                            expected[0] += sample * angle.cos();
                            expected[1] += sample * angle.sin();
                        }
                        let gains = panning_gains(count, width, pan);
                        let actual = oscillator.generate_sample(&input, 48_000.0, &gains);
                        for ch in 0..2 {
                            // 最大7音の合成誤差。単体係数の上限は専用テストで確認する。
                            assert!((actual[ch] - expected[ch]).abs() < 1e-6);
                        }
                        for gain in &gains[..count] {
                            assert!((gain[0] * gain[0] + gain[1] * gain[1] - 2.0).abs() < 1.18e-6);
                        }
                        assert_eq!(&oscillator.state.phases[count..], &before[count..]);
                        assert_eq!(
                            oscillator.state.phases[0],
                            440.0 * oscillator.state.frequency_ratios[0] / 48_000.0
                        );
                        if pan == -1.0 {
                            assert_eq!(actual[1], 0.0);
                        }
                        if pan == 1.0 {
                            assert_eq!(actual[0], 0.0);
                        }
                        if pan == 0.0 && (width == 0.0 || count == 1) {
                            assert_eq!(actual[0], actual[1]);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn interpolation_preserves_integer_boundaries_and_fractional_positions() {
        for table in [&tables::SINE[..], &tables::DETUNE[..]] {
            let end = (table.len() - 1) as f64;
            for i in 0..table.len() {
                let point = i as f64;
                for position in [
                    point.next_down().max(0.0),
                    point,
                    point.next_up().min(end),
                    (point + 0.5).min(end),
                ] {
                    let index = (position as usize).min(table.len() - 2);
                    let expected = table[index]
                        + (table[index + 1] - table[index]) * (position - index as f64);
                    // SAFETY: 201/1025要素の表に対し、有限の0〜最終位置を渡す。
                    let actual = unsafe { interpolate(position, table) };
                    assert_eq!(actual.to_bits(), expected.to_bits(), "{position}");
                }
            }
        }
    }

    #[test]
    fn phase_wraps_at_and_above_the_sample_rate() {
        for (frequency_hz, expected) in [
            (0.0, 0.0),
            (1.0, 0.25),
            (4.0_f64.next_down(), 1.0_f64.next_down()),
            (4.0, 0.0),
            (4.0_f64.next_up(), f64::EPSILON),
            (5.0, 0.25),
            (8.0, 0.0),
            (9.0, 0.25),
        ] {
            let mut oscillator = Oscillator::new();
            let input = OscillatorInput {
                frequency_hz,
                ..test_input()
            };
            assert!(oscillator.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0].is_finite());
            assert_eq!(oscillator.state.phases[0], expected, "{frequency_hz}");
        }
    }

    #[test]
    fn phase_modes_apply_only_at_note_on_and_cover_all_unison_phases() {
        let mut oscillator = Oscillator::new();
        oscillator.state.random = 42;
        oscillator.state.phases = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875];
        let mut input = test_input();
        input.frequency_hz = 0.0; // 境界処理後の位相を進めずに観測する。
        input.unison_voices = 7;
        input.detune_cents = 25.0;
        input.envelope.attack_ms = 0.0;
        input.envelope.decay_ms = 0.0;
        let held = oscillator.state.phases;
        oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7]);
        assert_eq!(oscillator.state.phases, held);
        input.phase_mode = PhaseMode::Zero;
        oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7]);
        assert_eq!(oscillator.state.phases, held);
        input.note_generation += 1;
        assert_eq!(
            oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        assert_eq!(oscillator.state.phases, [0.0; 7]);
        input.phase_mode = PhaseMode::Random;
        input.note_generation += 1;
        let sample = oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7])[0];
        let phases = oscillator.state.phases;
        for (i, &phase) in phases.iter().enumerate() {
            assert!((0.0..1.0).contains(&phase));
            assert!(!phases[..i].contains(&phase));
        }
        let expected =
            phases.into_iter().map(sine_sample).sum::<f64>() * 0.5 * std::f64::consts::SQRT_2
                / 7.0_f64.sqrt();
        assert!((sample - expected).abs() < 1e-15);
        let random = oscillator.state.random;
        oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7]);
        assert_eq!(oscillator.state.random, random);
        assert_eq!(oscillator.state.phases, phases);
        input.note_generation += 1;
        oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7]);
        assert_ne!(oscillator.state.phases, phases);
        // Note Offと処理停止では、新しいランダム位相を引かない。
        input.gate = false;
        input.reset_generation += 1;
        let phases = oscillator.state.phases;
        let random = oscillator.state.random;
        assert_eq!(
            oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        assert_eq!(oscillator.state.phases, phases);
        assert_eq!(oscillator.state.random, random);
    }

    #[test]
    fn random_phase_sequence_is_reproducible_from_its_state() {
        let mut oscillator = Oscillator::new();
        oscillator.state.random = 0;
        // SplitMix64の既知の先頭値。再開時も同じStateから同じ列を生成する。
        assert_eq!(
            oscillator.random_phase(),
            (0xe220_a839_7b1d_cdaf_u64 >> 11) as f64 / (1_u64 << 53) as f64
        );
        let state = oscillator.state;
        let expected: [f64; 7] = std::array::from_fn(|_| oscillator.random_phase());
        oscillator.state = state;
        assert_eq!(
            std::array::from_fn::<_, 7, _>(|_| oscillator.random_phase()),
            expected
        );
    }

    fn test_input() -> OscillatorInput {
        OscillatorInput {
            waveform: Waveform::Sine,
            phase_mode: PhaseMode::Keep,
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
        let mut oscillator = Oscillator::new();
        let input = OscillatorInput {
            frequency_hz: 0.0,
            ..test_input()
        };
        oscillator.generate_sample(&input, 1.0, &[[1.0; 2]; 7]);
        oscillator
    }

    #[test]
    fn inactive_samples_preserve_phase_but_new_notes_and_silent_attack_advance() {
        let mut oscillator = Oscillator::new();
        let mut input = test_input();
        input.frequency_hz = 100.0;
        input.gate = false;
        input.note_generation = 0;
        for _ in 0..16 {
            assert_eq!(
                oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0],
                0.0
            );
        }
        assert!(!oscillator.state.envelope.is_active());
        input.gate = true;
        input.note_generation += 1;
        assert_eq!(
            oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        assert!(oscillator.state.envelope.is_active());
        assert_eq!(oscillator.state.phases[0], 0.1);
        assert!(oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0] > 0.0);
        input.gate = false;
        input.envelope.release_ms = 0.0;
        assert_eq!(
            oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        let phase = oscillator.state.phases;
        for _ in 0..16 {
            assert_eq!(
                oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0],
                0.0
            );
        }
        assert_eq!(oscillator.state.phases, phase);
        input.gate = true;
        input.note_generation += 1;
        oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7]);
        assert_ne!(oscillator.state.phases, phase);
        input.gate = false;
        input.reset_generation += 1;
        assert_eq!(
            oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        assert_eq!(oscillator.state.reset_generation, input.reset_generation);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 密な補間誤差・実効値走査は通常のCPUテストで検証する。
    fn lookup_error_bounds_and_uncorrected_rms() {
        let (mut sine_error, mut detune_error, mut energy) = (0.0_f64, 0.0_f64, 0.0);
        let points = 200_003;
        for i in 0..points {
            let phase = i as f64 / points as f64;
            let value = sine_sample(phase);
            sine_error = sine_error.max((value - (std::f64::consts::TAU * phase).sin()).abs());
            energy += value * value;
            let cents = -100.0 + 200.0 * phase;
            detune_error =
                detune_error.max((detune_ratio(cents) / 2.0_f64.powf(cents / 1200.0) - 1.0).abs());
        }
        assert!(sine_error < 2.95e-7);
        assert!(detune_error < 4.18e-8);
        let rms = (energy / points as f64).sqrt() * 0.5 * std::f64::consts::SQRT_2;
        assert!((rms - 0.499_999_902).abs() < 1e-9);
    }

    #[test]
    fn table_endpoints_and_quadrant_edges_are_continuous() {
        for cents in -100..=100 {
            assert!(
                (detune_ratio(cents as f64) - 2.0_f64.powf(cents as f64 / 1200.0)).abs() < 1e-15
            );
        }
        assert_eq!(detune_ratio(0.0), 1.0);
        for edge in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            for phase in [edge.next_down(), edge, edge.next_up()] {
                if (0.0..1.0).contains(&phase) {
                    assert!(
                        (sine_sample(phase) - (std::f64::consts::TAU * phase).sin()).abs() < 1e-14
                    );
                }
            }
        }
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
            oscillator.generate_sample(&input, rate, &[[1.0; 2]; 7]);
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
                // 倍率補間の相対誤差4.18e-8が128サンプルの位相へ累積する範囲。
                assert!((actual - expected).abs() < 1e-7);
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
                        (state.generate_sample(&input, 8.0, &[[1.0; 2]; 7])[0] - peak * unit).abs()
                            < 1e-14,
                        "{waveform:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn triangle_and_square_match_piecewise_values_at_phase_boundaries() {
        for edge in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            for phase in [edge.next_down(), edge, edge.next_up()] {
                if !(0.0..1.0).contains(&phase) {
                    continue;
                }
                // 折り返しの式から独立した、区間ごとの直線で照合する。
                let triangle = if phase < 0.25 {
                    4.0 * phase
                } else if phase < 0.75 {
                    2.0 - 4.0 * phase
                } else {
                    4.0 * phase - 4.0
                };
                assert!((Waveform::Triangle.unit_sample(phase) - triangle).abs() <= f64::EPSILON);
                let square = if phase < 0.5 { 1.0 } else { -1.0 };
                assert_eq!(Waveform::Square.unit_sample(phase), square);
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
        assert_eq!(state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0], 0.0);
        input.waveform = Waveform::Triangle;
        assert!(
            (state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0] - 0.866_025_403_784_438_6).abs()
                < 1e-14
        );
        input.waveform = Waveform::Square;
        assert_eq!(state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0], -0.5);
        input.gate = false;
        input.reset_generation += 1;
        input.waveform = Waveform::Sawtooth;
        assert_eq!(state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0], 0.0);
        input.gate = true;
        // 波形/入力だけの検証は、位相を保持して即時attackで復帰させる。
        input.note_generation += 1;
        input.envelope.attack_ms = 0.0;
        input.envelope.decay_ms = 0.0;
        assert!(
            (state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0] - 0.433_012_701_892_219_3).abs()
                < 1e-14
        );
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
        assert_eq!(state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0], 0.0); // 更新前の位相0で生成
        assert!((state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0] - 0.5).abs() < 1e-14);
        // 入力だけを変更する。保持していた位相1/2から、新しい振幅・周波数で処理する。
        input.frequency_hz = 2.0;
        input.amplitude = 0.25 / std::f64::consts::SQRT_2;
        assert!(state.generate_sample(&input, 8.0, &[[1.0; 2]; 7])[0].abs() < 1e-14);
        input.gate = false;
        input.reset_generation += 1;
        for _ in 0..7 {
            assert_eq!(state.generate_sample(&input, 8.0, &[[1.0; 2]; 7])[0], 0.0);
        }
        input.gate = true;
        // 波形/入力だけの検証は、位相を保持して即時attackで復帰させる。
        input.note_generation += 1;
        input.envelope.attack_ms = 0.0;
        input.envelope.decay_ms = 0.0;
        assert!((state.generate_sample(&input, 8.0, &[[1.0; 2]; 7])[0] + 0.25).abs() < 1e-14);
        // 振幅0はゲート閉とは異なり、状態更新を続ける。
        input.amplitude = 0.0;
        assert_eq!(state.generate_sample(&input, 8.0, &[[1.0; 2]; 7])[0], 0.0);
        input.amplitude = 0.5 / std::f64::consts::SQRT_2;
        assert!((state.generate_sample(&input, 8.0, &[[1.0; 2]; 7])[0] - 0.5).abs() < 1e-14);
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
                state.generate_sample(&input, 48000.0, &[[1.0; 2]; 7]);
                for (k, phase) in expected_phases.iter_mut().enumerate().take(voices as usize) {
                    let cents = if voices == 1 {
                        0.0
                    } else {
                        -100.0 + 200.0 * k as f64 / f64::from(voices - 1)
                    };
                    *phase = (*phase + input.frequency_hz * 2.0_f64.powf(cents / 1200.0) / 48000.0)
                        .fract();
                    assert!((state.state.phases[k] - *phase).abs() < 3e-8);
                }
            }
            let held_phases = state.state.phases;
            input.gate = false;
            input.reset_generation += 1;
            state.generate_sample(&input, 48000.0, &[[1.0; 2]; 7]);
            assert_eq!(state.state.phases, held_phases);
            // 0 centに切り替えると、過去に異なる位相へ進んでいても先頭だけで生成。
            input.detune_cents = 0.0;
            input.gate = true;
            input.note_generation += 1;
            input.envelope.attack_ms = 0.0;
            let expected = 0.5 * std::f64::consts::SQRT_2 * sine_sample(held_phases[0]);
            assert!(
                (state.generate_sample(&input, 48000.0, &[[1.0; 2]; 7])[0] - expected).abs()
                    < 1e-12
            );
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
                        let sample = state.generate_sample(&input, 24000.0, &[[1.0; 2]; 7])[0];
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
        state.generate_sample(&input, 4.0, &[[1.0; 2]; 7]); // 位相1/4を保持しておく。
        for frequency_hz in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let invalid = OscillatorInput {
                frequency_hz,
                ..input
            };
            assert!(!invalid.is_valid());
            assert_eq!(state.generate_sample(&invalid, 4.0, &[[1.0; 2]; 7])[0], 0.0);
        }
        for amplitude in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            let invalid = OscillatorInput { amplitude, ..input };
            assert!(!invalid.is_valid());
            assert_eq!(state.generate_sample(&invalid, 4.0, &[[1.0; 2]; 7])[0], 0.0);
        }
        for unison_voices in [0, 8, u32::MAX] {
            assert_eq!(
                state.generate_sample(
                    &OscillatorInput {
                        unison_voices,
                        ..input
                    },
                    4.0,
                    &[[1.0; 2]; 7]
                )[0],
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
                    4.0,
                    &[[1.0; 2]; 7]
                )[0],
                0.0
            );
        }
        for rate in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(state.generate_sample(&input, rate, &[[1.0; 2]; 7])[0], 0.0);
        }
        assert!((state.generate_sample(&input, 4.0, &[[1.0; 2]; 7])[0] - 0.5).abs() < 1e-14);
        let extreme = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: f64::MAX,
            amplitude: 1.0,
            gate: true,
            ..test_input()
        };
        for rate in [f64::MIN_POSITIVE, 44100.0, 96000.0, f64::MAX] {
            for _ in 0..16 {
                assert!(state.generate_sample(&extreme, rate, &[[1.0; 2]; 7])[0].is_finite());
            }
        }
    }
}

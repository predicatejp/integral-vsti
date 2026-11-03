// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 発振器回路の定常入力と内部状態を分離した単一のオシレータ。

use crate::random::Random;
#[path = "oscillator/adaa.rs"]
mod adaa;

#[cfg(test)]
#[path = "oscillator/reference.rs"]
pub(crate) mod reference;
#[path = "oscillator_tables.rs"]
mod tables;
#[path = "oscillator/wavetable.rs"]
mod wavetable;

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

/// 周期波4種と一様分布のホワイトノイズ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    /// 位相0から正方向に立ち上がるサイン波。
    Sine,
    /// 位相0で0、1/4で正のピーク、3/4で負のピークとなる三角波。
    Triangle,
    /// 前半周期が正、後半周期が負の50%デューティ矩形波。
    Square,
    /// 上昇するノコギリ波。帯域制限により周期境界の不連続点は0を通る。
    Sawtooth,
    /// 毎サンプル別の一様乱数を使うホワイトノイズ。
    Noise,
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
/// 音量AHDSRはVoiceが管理し、activeはその発音期間を示す。
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
    /// 目標周波数（Hz、非負の有限値）。入力を変更しても位相は初期化しない。
    pub frequency_hz: f64,
    /// 波形を読む位置への位相変調（周期単位）。有限のi32範囲を受け取り、保持位相には累積しない。
    pub phase_modulation: f64,
    /// 位相変調が有効な周期波には2階ADAAを適用する。瞬間的に変調量0でも有効。
    pub phase_modulated: bool,
    /// 音量AHDSRの発音期間。release中もtrue、終了・停止後はfalse。
    pub active: bool,
    /// Note Onごとに増える世代番号。同音高の再トリガーでも位相モードを適用する。
    pub note_generation: u64,
}

impl OscillatorInput {
    pub fn rendered_voices(&self) -> usize {
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
        (0.0..=f64::MAX).contains(&self.frequency_hz)
            & (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&self.phase_modulation)
            & (1..=7).contains(&self.unison_voices)
            & (0.0..=100.0).contains(&self.detune_cents)
    }
}

/// 発振器回路の内部状態。イベント処理からは書き換えない。
#[derive(Clone, Copy)]
pub struct OscillatorState {
    /// 次に生成するサンプルの位相（周期単位、0以上1未満。境界の丸めで1となる場合もある）。
    /// 初期値は0。Note On時はphase_modeに従い、release完了後と処理停止中は進めない。
    phases: [f64; 7],
    /// 同じ位相原点で表した直前2点の変調後位相。
    history: [[f64; 2]; 7],
    /// 現在の周波数に対応する原始関数テーブル。
    bank: [adaa::Bank; 7],
    /// 履歴を連続使用できるか。ノート・波形・発音数変更時に破棄する。
    valid: bool,
    /// 履歴に有効なユニゾン数。未使用スロットの再利用を検出する。
    history_count: usize,
    /// Note On時だけ進める位相用乱数。ノイズ用とは独立した状態を持つ。
    random: Random,
    /// ノイズ専用xoshiro256+の状態。各ユニゾンに連続する別の値を割り当てる。
    noise: Random,
    /// サンプル処理へ反映済みのNote On世代番号。
    note_generation: u64,
    /// 周波数倍率の計算に使った（ユニゾン数, Detune）。Noneは未計算。
    unison_input: Option<(u32, f64)>,
    /// 位相増分・帯域選択の計算に使った（基音Hz, サンプルレート, 波形）。
    frequency_input: Option<(f64, f64, Waveform)>,
    /// 各ユニゾンの帯域とクロスフェード係数。
    bands: [wavetable::Selection; 7],
    /// 各ユニゾンの基音に対する周波数倍率。入力の音程幅が変わったときだけ更新する。
    frequency_ratios: [f64; 7],
    /// 各ユニゾンの1サンプル当たりの位相増分。周波数・レート変更時も更新する。
    phase_increments: [f64; 7],
}

/// 同じサンプル時刻の、音量・定位適用前のユニゾン波形。
pub struct OscillatorSample {
    /// 理想波形のピーク振幅1を基準とする生波形。帯域制限のオーバーシュートを含み、未使用要素は0。
    pub values: [f64; 7],
    /// 発音中の要素数（0〜7）。変調源のOsc2/3は最大1。
    pub count: usize,
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
                history: [[0.; 2]; 7],
                bank: [adaa::Bank::new(Waveform::Sine, 0.); 7],
                valid: false,
                history_count: 0,
                random: Random::new(),
                noise: Random::new(),
                note_generation: 0,
                unison_input: None,
                frequency_input: None,
                bands: [wavetable::Selection::EMPTY; 7],
                frequency_ratios: [1.0; 7],
                phase_increments: [0.0; 7],
            },
        }
    }

    /// 現在の入力・状態でサンプルを生成し、その後で次のサンプルに向けて状態を更新する。
    /// sample_rateは処理環境のサンプルレート（Hz）。不正な入力・レートでは無音を返し、状態を保持する。
    #[inline]
    pub fn generate_sample(
        &mut self,
        input: &OscillatorInput,
        sample_rate: f64,
    ) -> OscillatorSample {
        self.generate_optional_sample::<true>(input, sample_rate)
    }

    /// 音声・変調のどちらにも使わないサンプルの状態だけを更新する。
    #[inline]
    pub fn advance_sample(&mut self, input: &OscillatorInput, sample_rate: f64) {
        self.generate_optional_sample::<false>(input, sample_rate);
    }

    #[inline]
    fn generate_optional_sample<const OUTPUT: bool>(
        &mut self,
        input: &OscillatorInput,
        sample_rate: f64,
    ) -> OscillatorSample {
        if !(input.is_valid() & (sample_rate <= f64::MAX) & (sample_rate > 0.0)) {
            return OscillatorSample {
                values: [0.0; 7],
                count: 0,
            };
        }
        // ノート境界と位相モードを反映して生成した後、次サンプルへ進める。
        // キャッシュを含むState全体のサンプルごとのコピーは行わない。
        if input.note_generation != self.state.note_generation && input.active {
            self.state.valid = false;
            match input.phase_mode {
                PhaseMode::Keep => {}
                PhaseMode::Zero => self.state.phases.fill(0.0),
                PhaseMode::Random => {
                    for k in 0..self.state.phases.len() {
                        self.state.phases[k] = self.state.random.next_f64();
                    }
                }
            }
        }
        self.state.note_generation = input.note_generation;
        // activeはVoiceの共通AHDSRから渡す。無音のattack中も位相を進める。
        if !input.active {
            self.state.valid = false;
            return OscillatorSample {
                values: [0.0; 7],
                count: 0,
            };
        }
        self.refresh_frequency(input, sample_rate);
        let sample = self.generate_waveform_sample::<OUTPUT>(input);
        self.update_sample(input);
        sample
    }

    /// 位相変調は読出し位置だけへ適用する。Noiseはユニゾンごとに乱数を一つ進める。
    #[inline]
    fn generate_waveform_sample<const OUTPUT: bool>(
        &mut self,
        input: &OscillatorInput,
    ) -> OscillatorSample {
        match input.waveform {
            Waveform::Noise => {
                self.state.valid = false;
                let count = input.rendered_voices();
                let mut values = [0.0; 7];
                for value in &mut values[..count] {
                    *value = 2.0 * self.state.noise.next_f64() - 1.0;
                }
                OscillatorSample { values, count }
            }
            Waveform::Sine => self.generate_periodic_sample::<false, OUTPUT>(input),
            _ => self.generate_bandlimited_sample::<OUTPUT>(input),
        }
    }

    // 帯域別のループを分離してSine/Noiseを小さく保ち、ユニゾンごとの関数呼出しを省く。
    #[inline(never)]
    fn generate_bandlimited_sample<const OUTPUT: bool>(
        &mut self,
        input: &OscillatorInput,
    ) -> OscillatorSample {
        self.generate_periodic_sample::<true, OUTPUT>(input)
    }

    /// 波形別に展開する周期波の読出し。位相変調の処理は両経路で共通にする。
    #[inline(always)]
    fn generate_periodic_sample<const BANDED: bool, const OUTPUT: bool>(
        &mut self,
        input: &OscillatorInput,
    ) -> OscillatorSample {
        let count = input.rendered_voices();
        let mut values = [0.0; 7];
        if input.phase_modulated {
            return self.generate_antialiased_sample::<OUTPUT>(input);
        }
        self.state.valid = false;
        if !OUTPUT {
            return OscillatorSample { values, count };
        }
        // 整数周期を先に除き、±1/±2回転では元の位相を厳密に保持する。
        // SAFETY: generate_sampleで位相変調が有限のi32範囲であることを検証済み。
        let cycles = unsafe { input.phase_modulation.to_int_unchecked::<i32>() };
        let offset = input.phase_modulation - f64::from(cycles);
        if offset == 0.0 {
            for (k, (value, &phase)) in values[..count]
                .iter_mut()
                .zip(&self.state.phases)
                .enumerate()
            {
                *value = if BANDED {
                    self.state.bands[k].generate_sample(input.waveform, phase)
                } else {
                    sine_sample(phase)
                };
            }
        } else {
            for (k, (value, &phase)) in values[..count]
                .iter_mut()
                .zip(&self.state.phases)
                .enumerate()
            {
                let position = phase + offset;
                let wrapped = position + f64::from((position < 0.0) as u8)
                    - f64::from((position >= 1.0) as u8);
                *value = if BANDED {
                    self.state.bands[k].generate_sample(input.waveform, wrapped)
                } else {
                    sine_sample(wrapped)
                };
            }
        }
        OscillatorSample { values, count }
    }

    #[inline(never)]
    fn generate_antialiased_sample<const OUTPUT: bool>(
        &mut self,
        input: &OscillatorInput,
    ) -> OscillatorSample {
        let count = input.rendered_voices();
        let mut values = [0.; 7];
        if self.state.history_count != count {
            self.state.valid = false;
            self.state.history_count = count;
        }
        for (k, value) in values[..count].iter_mut().enumerate() {
            let phase = self.state.phases[k];
            let q = phase + input.phase_modulation;
            let inc = self.state.phase_increments[k];
            let [b, c] = if self.state.valid {
                self.state.history[k]
            } else {
                [q - inc, q - 2. * inc]
            };
            if OUTPUT {
                *value = self.state.bank[k].generate_sample(q, b, c);
            }
            // 次サンプルで保持位相が巻き戻る量を履歴にも適用する。
            // 変調量は折り返さず、周期を跨ぐ差商の距離を保持する。
            let wrap = f64::from((phase + inc >= 1.) as u8);
            self.state.history[k] = [q - wrap, b - wrap];
        }
        self.state.valid = true;
        OscillatorSample { values, count }
    }

    #[cfg(test)]
    fn read_sample(&self, wave: Waveform, phase: f64, k: usize) -> f64 {
        match wave {
            Waveform::Sine => sine_sample(phase),
            Waveform::Noise => 2.0 * phase - 1.0,
            _ => self.state.bands[k].generate_sample(wave, phase),
        }
    }

    /// 定常入力が変わったサンプルでだけ周波数倍率・位相増分を再計算する。
    #[inline]
    fn refresh_frequency(&mut self, input: &OscillatorInput, sample_rate: f64) {
        let unison = (input.unison_voices, input.detune_cents);
        let changed = self.state.unison_input != Some(unison);
        let count = input.rendered_voices();
        if changed {
            for (k, ratio) in self.state.frequency_ratios[..count].iter_mut().enumerate() {
                *ratio = input.frequency_ratio(k);
            }
            self.state.unison_input = Some(unison);
        }
        let frequency = (input.frequency_hz, sample_rate, input.waveform);
        if changed || self.state.frequency_input != Some(frequency) {
            if self
                .state
                .frequency_input
                .is_some_and(|old| old.2 != input.waveform)
            {
                self.state.valid = false;
            }
            for (k, (increment, ratio)) in self.state.phase_increments[..count]
                .iter_mut()
                .zip(&self.state.frequency_ratios[..count])
                .enumerate()
            {
                let hz = (input.frequency_hz * ratio).min(f64::MAX);
                if !matches!(input.waveform, Waveform::Sine | Waveform::Noise) {
                    self.state.bands[k] = wavetable::Selection::new(
                        input.waveform,
                        if hz < sample_rate {
                            hz / sample_rate
                        } else {
                            1.0
                        },
                    );
                }
                self.state.bank[k] = adaa::Bank::new(input.waveform, (hz / sample_rate).min(1.0));
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
    fn update_sample(&mut self, input: &OscillatorInput) {
        // generate_sampleが発音中であることを確認してから呼ぶ。
        for (phase, increment) in self.state.phases[..input.rendered_voices()]
            .iter_mut()
            .zip(&self.state.phase_increments)
        {
            // 増分は[0, 1)、和は[0, 2)。周期境界を1回だけ補正する。
            let next = *phase + increment;
            *phase = next - f64::from((next >= 1.0) as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDLIMITED: [Waveform; 3] = [Waveform::Triangle, Waveform::Square, Waveform::Sawtooth];

    #[test]
    #[cfg_attr(miri, ignore)] // 多数のFourier係数との数値照合は通常のCPUテストで行う。
    fn bandlimited_samples_match_retained_fourier_harmonics() {
        for waveform in BANDLIMITED {
            for rate in [44100.0, 48000.0, 96000.0] {
                for frequency_hz in [440.0, 8000.0, 18000.0] {
                    let input = OscillatorInput {
                        waveform,
                        frequency_hz,
                        ..test_input()
                    };
                    let mut oscillator = Oscillator::new();
                    for _ in 0..257 {
                        let phase = oscillator.state.phases[0];
                        let actual = oscillator.generate_sample(&input, rate).values[0];
                        let expected =
                            reference::audio_sample(waveform, phase, frequency_hz / rate);
                        assert!(
                            (actual - expected).abs() < 5e-6,
                            "{waveform:?}, {rate}, {frequency_hz}: {actual} != {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 全帯域の密な位相走査は通常のCPUテストで行う。
    fn bands_crossfade_before_nyquist_without_discontinuities() {
        for wave in BANDLIMITED {
            for band in 0..=28 {
                let h = 2.0_f64.powf(f64::from(band) / 2.0).floor();
                for fraction in [0.9, 0.95, 1.0] {
                    let boundary = 0.5 / h * fraction;
                    let left = wavetable::Selection::new(wave, boundary * (1.0 - 1e-10));
                    let right = wavetable::Selection::new(wave, boundary * (1.0 + 1e-10));
                    for n in 0..=257 {
                        let phase = f64::from(n) / 257.0;
                        let a = left.generate_sample(wave, phase);
                        let b = right.generate_sample(wave, phase);
                        assert!(a.is_finite() && b.is_finite());
                        assert!((a - b).abs() < 1e-8);
                    }
                }
            }
            let mut oscillator = Oscillator::new();
            for frequency_hz in [24000.0, 48000.0, 96000.0, f64::MAX] {
                let input = OscillatorInput {
                    waveform: wave,
                    frequency_hz,
                    ..test_input()
                };
                for _ in 0..8 {
                    assert_eq!(oscillator.generate_sample(&input, 48000.0).values[0], 0.0);
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // スイープのFourier照合は通常のCPUテストで行う。
    fn frequency_sweeps_cross_bands_without_holding_the_old_table() {
        for wave in BANDLIMITED {
            for harmonic in [2, 4, 8, 16, 32, 64, 128] {
                for reverse in [false, true] {
                    let mut oscillator = Oscillator::new();
                    let mut phase = 0.137;
                    oscillator.state.phases[0] = phase;
                    for n in 0..129 {
                        let position = f64::from(if reverse { 128 - n } else { n }) / 128.0;
                        let increment = (0.85 + 0.20 * position) * 0.5 / f64::from(harmonic);
                        let input = OscillatorInput {
                            waveform: wave,
                            frequency_hz: increment * 48000.0,
                            ..test_input()
                        };
                        let actual = oscillator.generate_sample(&input, 48000.0).values[0];
                        let expected = reference::audio_sample(wave, phase, increment);
                        assert!((actual - expected).abs() < 5e-6);
                        phase = (phase + input.frequency_hz / 48000.0).fract();
                    }
                }
            }
        }
    }

    #[test]
    fn unison_selects_each_band_after_detune_and_refreshes_on_input_changes() {
        let mut combined = Oscillator::new();
        let mut singles: [Oscillator; 7] = std::array::from_fn(|_| Oscillator::new());
        for n in 0..96 {
            let input = OscillatorInput {
                waveform: [
                    Waveform::Sawtooth,
                    Waveform::Triangle,
                    Waveform::Square,
                    Waveform::Sine,
                ][n / 24],
                frequency_hz: if n % 2 == 0 { 750.0 } else { 3000.0 },
                unison_voices: 7,
                detune_cents: 1.0 + n as f64,
                ..test_input()
            };
            let rate = if n % 3 == 0 { 96000.0 } else { 48000.0 };
            let actual = combined.generate_sample(&input, rate);
            for (k, single) in singles.iter_mut().enumerate() {
                let individual = OscillatorInput {
                    frequency_hz: input.frequency_hz * input.frequency_ratio(k),
                    unison_voices: 1,
                    detune_cents: 0.0,
                    ..input
                };
                let expected = single.generate_sample(&individual, rate);
                assert_eq!(actual.values[k], expected.values[0]);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 非同期位相でのDFT走査は通常のCPUテストで行う。
    fn bandlimited_output_rejects_nonharmonic_energy() {
        let length = 1021; // テーブルの区間数と互いに素にし、補間点の間も検査する。
        let fundamental = 91;
        for wave in BANDLIMITED {
            let mut oscillator = Oscillator::new();
            let input = OscillatorInput {
                waveform: wave,
                frequency_hz: 48000.0 * fundamental as f64 / length as f64,
                ..test_input()
            };
            let samples: Vec<_> = (0..length)
                .map(|_| oscillator.generate_sample(&input, 48000.0).values[0])
                .collect();
            let mut harmonic_energy = 0.0;
            let mut unwanted_energy = 0.0;
            for bin in 0..=length / 2 {
                let (mut real, mut imaginary) = (0.0, 0.0);
                for (n, &sample) in samples.iter().enumerate() {
                    let angle = std::f64::consts::TAU * (bin * n) as f64 / length as f64;
                    real += sample * angle.cos();
                    imaginary += sample * angle.sin();
                }
                let energy = real * real + imaginary * imaginary;
                if bin > 0
                    && bin % fundamental == 0
                    && (wave == Waveform::Sawtooth || bin / fundamental % 2 == 1)
                {
                    harmonic_energy += energy;
                } else {
                    unwanted_energy += energy;
                }
            }
            assert!(unwanted_energy / harmonic_energy < 1e-12, "{wave:?}");
        }
    }

    // 音量・定位との接続を検証するため、独立した音量処理を通す。
    fn audio_sample(
        oscillator: &mut Oscillator,
        input: &OscillatorInput,
        rate: f64,
        gains: &PanningGains,
    ) -> [f64; 2] {
        let sample = oscillator.generate_sample(input, rate);
        if sample.count == 0 {
            return [0.0; 2];
        }
        crate::volume::process_sample(
            &sample.values[..sample.count],
            gains,
            0.5 * crate::volume::gain(input.waveform, sample.count, 1.0),
        )
    }

    #[test]
    fn phase_modulation_wraps_without_changing_stored_phase() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for offset in [
                f64::from(i32::MIN),
                -2.5,
                -2.0,
                -1.75,
                -1.0,
                -0.25,
                0.0,
                0.25,
                1.0,
                1.75,
                2.0,
                2.5,
                f64::from(i32::MAX),
            ] {
                let mut oscillator = Oscillator::new();
                oscillator.state.phases = [0.0, 0.125, 0.25, 0.375, 0.5, 0.75, 0.875];
                let before = oscillator.state.phases;
                let input = OscillatorInput {
                    waveform: wave,
                    unison_voices: 7,
                    detune_cents: 25.0,
                    phase_modulation: offset,
                    ..test_input()
                };
                let result = oscillator.generate_sample(&input, 48000.0);
                assert_eq!(result.count, 7);
                for (k, phase) in before.into_iter().enumerate() {
                    let expected =
                        oscillator.read_sample(wave, (phase + offset).rem_euclid(1.0), k);
                    assert_eq!(result.values[k], expected);
                    assert!(result.values[k].is_finite());
                    assert_eq!(
                        oscillator.state.phases[k],
                        phase + oscillator.state.phase_increments[k]
                    );
                }
            }
        }
    }

    #[test]
    fn noise_is_unit_amplitude_and_phase_modulation_preserves_its_sequence() {
        let mut oscillator = Oscillator::new();
        oscillator.state.noise = Random::from_seed(NOISE_SEED);
        let mut random = oscillator.state.noise;
        for offset in [-2.0, -0.37, 0.0, 0.61, 2.0] {
            let input = OscillatorInput {
                waveform: Waveform::Noise,
                phase_modulation: offset,
                unison_voices: 7,
                detune_cents: 25.0,
                ..test_input()
            };
            let result = oscillator.generate_sample(&input, 48000.0);
            for value in result.values {
                assert_eq!(value, 2.0 * random.next_f64() - 1.0);
                assert!((-1.0..=1.0).contains(&value));
            }
            assert_eq!(oscillator.state.noise, random);
        }
    }

    const NOISE_SEED: [u64; 4] = [
        0x1234_5678_9abc_def0,
        0xfedc_ba98_7654_3210,
        0x1020_3040_5060_7080,
        0x8877_6655_4433_2211,
    ];

    #[test]
    fn noise_controls_preserve_sequence_and_stop_when_inactive() {
        let mut input = test_input();
        input.waveform = Waveform::Noise;
        let mut initial = sustained();
        initial.state.noise = Random::from_seed(NOISE_SEED);
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
                            audio_sample(&mut actual, &a, 96000.0, &gains),
                            audio_sample(&mut reference, &b, 44100.0, &gains)
                        );
                        assert_eq!(reference.state.random, initial.state.random);
                    }
                }
            }
        }
        let mut oscillator = initial;
        let gains = panning_gains(1, 1.0, 0.0);
        audio_sample(&mut oscillator, &input, 48000.0, &gains);
        input.active = false;
        for _ in 0..49 {
            audio_sample(&mut oscillator, &input, 48000.0, &gains);
        }
        let noise = oscillator.state.noise;
        assert_eq!(
            audio_sample(&mut oscillator, &input, 48000.0, &gains),
            [0.0; 2]
        );
        assert_eq!(oscillator.state.noise, noise);
        input.active = true;
        input.note_generation += 1;
        audio_sample(&mut oscillator, &input, 48000.0, &gains);
        assert_ne!(oscillator.state.noise, noise);
        input.active = false;
        let noise = oscillator.state.noise;
        assert_eq!(
            audio_sample(&mut oscillator, &input, 48000.0, &gains),
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
                oscillator.state.noise = Random::from_seed(NOISE_SEED);
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
                    let sample = audio_sample(&mut oscillator, &input, 48000.0, &gains);
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
                            let sample = sine_sample(phase) / (count as f64).sqrt();
                            expected[0] += sample * angle.cos();
                            expected[1] += sample * angle.sin();
                        }
                        let gains = panning_gains(count, width, pan);
                        let actual = audio_sample(&mut oscillator, &input, 48_000.0, &gains);
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
            assert!(audio_sample(&mut oscillator, &input, 4.0, &[[1.0; 2]; 7])[0].is_finite());
            assert_eq!(oscillator.state.phases[0], expected, "{frequency_hz}");
        }
    }

    #[test]
    fn phase_modes_apply_only_at_note_on_and_cover_all_unison_phases() {
        let mut oscillator = Oscillator::new();
        oscillator.state.random = Random::from_seed(NOISE_SEED);
        oscillator.state.phases = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875];
        let mut input = test_input();
        input.frequency_hz = 0.0; // 境界処理後の位相を進めずに観測する。
        input.unison_voices = 7;
        input.detune_cents = 25.0;
        let held = oscillator.state.phases;
        audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7]);
        assert_eq!(oscillator.state.phases, held);
        input.phase_mode = PhaseMode::Zero;
        audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7]);
        assert_eq!(oscillator.state.phases, held);
        input.note_generation += 1;
        assert_eq!(
            audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        assert_eq!(oscillator.state.phases, [0.0; 7]);
        input.phase_mode = PhaseMode::Random;
        input.note_generation += 1;
        let sample = audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7])[0];
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
        audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7]);
        assert_eq!(oscillator.state.random, random);
        assert_eq!(oscillator.state.phases, phases);
        input.note_generation += 1;
        audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7]);
        assert_ne!(oscillator.state.phases, phases);
        // Note Offと処理停止では、新しいランダム位相を引かない。
        input.active = false;
        let phases = oscillator.state.phases;
        let random = oscillator.state.random;
        assert_eq!(
            audio_sample(&mut oscillator, &input, 48_000.0, &[[1.0; 2]; 7])[0],
            0.0
        );
        assert_eq!(oscillator.state.phases, phases);
        assert_eq!(oscillator.state.random, random);
    }

    #[test]
    fn smoothing_tracks_unwrapped_phase_and_resets_new_unison_slots() {
        let mut oscillator = Oscillator::new();
        let mut input = OscillatorInput {
            frequency_hz: 876.0,
            phase_modulated: true,
            ..test_input()
        };
        let bank = adaa::Bank::new(Waveform::Sine, 876.0 / 48000.0);
        let increment = input.frequency_hz / 48000.0;
        let mut history = [-increment, -2.0 * increment];
        for n in 0..4096 {
            input.phase_modulation = 2.0 * (n as f64 * 0.183).sin();
            let q = n as f64 * increment + input.phase_modulation;
            let expected = bank.generate_sample(q, history[0], history[1]);
            let actual = oscillator.generate_sample(&input, 48000.0).values[0];
            assert!(
                (actual - expected).abs() < 1e-7,
                "{n}: {actual} != {expected}"
            );
            history = [q, history[0]];
        }
        for count in [7, 2, 6, 1] {
            input.unison_voices = count;
            input.detune_cents = 25.0;
            let old_phases = oscillator.state.phases;
            let actual = oscillator.generate_sample(&input, 48000.0);
            for (k, phase) in old_phases.iter().enumerate().take(actual.count) {
                let q = phase + input.phase_modulation;
                let inc = oscillator.state.phase_increments[k];
                let bank = oscillator.state.bank[k];
                let expected = bank.generate_sample(q, q - inc, q - 2.0 * inc);
                assert!((actual.values[k] - expected).abs() < 1e-9);
            }
            // Detune automation preserves history for the existing slots.
            input.detune_cents = 26.0;
            oscillator.generate_sample(&input, 48000.0);
            assert!(oscillator.state.valid);
        }
        input.unison_voices = 1;
        input.detune_cents = 0.0;
        input.phase_modulation = 0.0;
        input.phase_modulated = false;
        let phase = oscillator.state.phases[0];
        assert_eq!(
            oscillator.generate_sample(&input, 48000.0).values[0],
            sine_sample(phase)
        );
        assert!(!oscillator.state.valid);
        input.phase_modulated = true;
        input.phase_modulation = 0.3;
        let q = oscillator.state.phases[0] + input.phase_modulation;
        let expected = bank.generate_sample(q, q - increment, q - 2.0 * increment);
        assert!((oscillator.generate_sample(&input, 48000.0).values[0] - expected).abs() < 1e-9);
    }

    fn test_input() -> OscillatorInput {
        OscillatorInput {
            waveform: Waveform::Sine,
            phase_mode: PhaseMode::Keep,
            unison_voices: 1,
            detune_cents: 0.0,
            frequency_hz: 1.0,
            phase_modulation: 0.0,
            phase_modulated: false,
            active: true,
            note_generation: 1,
        }
    }

    // 位相0のまま、最初のNote On世代を反映してから検証する。
    fn sustained() -> Oscillator {
        let mut oscillator = Oscillator::new();
        let input = OscillatorInput {
            frequency_hz: 0.0,
            ..test_input()
        };
        audio_sample(&mut oscillator, &input, 1.0, &[[1.0; 2]; 7]);
        oscillator
    }

    #[test]
    fn advancing_without_output_preserves_phase_noise_and_adaa_history() {
        let mut full = Oscillator::new();
        let mut skipped = Oscillator { state: full.state };
        let waves = [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
            Waveform::Noise,
        ];
        for n in 0..5000 {
            let mut input = test_input();
            input.waveform = waves[n / 61 % waves.len()];
            input.unison_voices = (n / 79 % 7 + 1) as u32;
            input.detune_cents = if n % 11 == 0 { 0.0 } else { 25.0 };
            input.phase_mode = [PhaseMode::Keep, PhaseMode::Zero, PhaseMode::Random][n / 127 % 3];
            input.note_generation = (n / 127) as u64;
            input.active = n % 43 != 0;
            input.phase_modulation = (n as f64 * 0.17).sin() * 2.0;
            input.phase_modulated = n % 17 != 0;
            input.frequency_hz = 40.0 + (n % 300) as f64 * 17.0;
            let expected = full.generate_sample(&input, 48000.0);
            if n % 19 == 0 {
                let actual = skipped.generate_sample(&input, 48000.0);
                assert_eq!(actual.count, expected.count);
                assert_eq!(actual.values, expected.values);
            } else {
                skipped.advance_sample(&input, 48000.0);
            }
            assert_eq!(skipped.state.phases, full.state.phases);
            assert_eq!(skipped.state.history, full.state.history);
            assert_eq!(skipped.state.valid, full.state.valid);
            assert_eq!(skipped.state.random, full.state.random);
            assert_eq!(skipped.state.noise, full.state.noise);
        }
    }

    #[test]
    fn inactive_samples_preserve_phase_and_silent_active_samples_advance() {
        let mut oscillator = Oscillator::new();
        let mut input = test_input();
        input.frequency_hz = 100.0;
        input.active = false;
        for _ in 0..16 {
            assert_eq!(
                audio_sample(&mut oscillator, &input, 1000.0, &[[1.0; 2]; 7]),
                [0.0; 2]
            );
        }
        assert_eq!(oscillator.state.phases, [0.0; 7]);
        input.active = true;
        input.note_generation += 1;

        assert_eq!(
            audio_sample(&mut oscillator, &input, 1000.0, &[[1.0; 2]; 7]),
            [0.0; 2]
        );
        assert_eq!(oscillator.state.phases[0], 0.1);

        assert!(audio_sample(&mut oscillator, &input, 1000.0, &[[1.0; 2]; 7])[0] > 0.0);
        input.active = false;
        let phases = oscillator.state.phases;
        for _ in 0..16 {
            assert_eq!(
                audio_sample(&mut oscillator, &input, 1000.0, &[[1.0; 2]; 7]),
                [0.0; 2]
            );
        }
        assert_eq!(oscillator.state.phases, phases);
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
            audio_sample(&mut oscillator, &input, rate, &[[1.0; 2]; 7]);
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
    fn waveform_changes_and_gate_preserve_phase() {
        let mut oscillator = sustained();
        let mut input = test_input();
        for (n, wave) in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
            Waveform::Sine,
        ]
        .into_iter()
        .enumerate()
        {
            input.waveform = wave;
            input.active = n != 3;
            let before = oscillator.state.phases;
            oscillator.generate_sample(&input, 8.0);
            assert_eq!(
                oscillator.state.phases[0],
                before[0] + if input.active { 0.125 } else { 0.0 }
            );
            if input.active {
                let sample = oscillator.read_sample(wave, before[0], 0);
                assert!(sample.is_finite());
            }
        }
    }

    #[test]
    fn frequency_changes_preserve_phase_and_modulation_is_not_accumulated() {
        let mut oscillator = sustained();
        let mut input = test_input();
        assert_eq!(oscillator.generate_sample(&input, 4.0).values[0], 0.0);
        input.frequency_hz = 2.0;
        input.phase_modulation = 0.25;
        assert_eq!(oscillator.generate_sample(&input, 8.0).values[0], 0.0);
        input.phase_modulation = 0.0;
        assert_eq!(oscillator.generate_sample(&input, 8.0).values[0], 0.0);
        assert_eq!(oscillator.generate_sample(&input, 8.0).values[0], -1.0);
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
                audio_sample(&mut state, &input, 48000.0, &[[1.0; 2]; 7]);
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
            input.active = false;
            audio_sample(&mut state, &input, 48000.0, &[[1.0; 2]; 7]);
            assert_eq!(state.state.phases, held_phases);
            // 0 centに切り替えると、過去に異なる位相へ進んでいても先頭だけで生成。
            input.detune_cents = 0.0;
            input.active = true;
            input.note_generation += 1;
            let expected = 0.5 * std::f64::consts::SQRT_2 * sine_sample(held_phases[0]);
            assert!(
                (audio_sample(&mut state, &input, 48000.0, &[[1.0; 2]; 7])[0] - expected).abs()
                    < 1e-12
            );
            assert_eq!(&state.state.phases[1..], &held_phases[1..]);
        }
    }

    #[test]
    #[cfg(not(miri))] // 長時間平均の数値検証は通常のRust実行で行う。
    fn unison_rms_stays_near_ideal_at_440_hz() {
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
                        let sample = audio_sample(&mut state, &input, 24000.0, &[[1.0; 2]; 7])[0];
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
            phase_modulation: 0.0,
            phase_modulated: false,
            active: true,
            ..test_input()
        };
        audio_sample(&mut state, &input, 4.0, &[[1.0; 2]; 7]); // 位相1/4を保持しておく。
        for frequency_hz in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let invalid = OscillatorInput {
                frequency_hz,
                ..input
            };
            assert!(!invalid.is_valid());
            assert_eq!(
                audio_sample(&mut state, &invalid, 4.0, &[[1.0; 2]; 7])[0],
                0.0
            );
        }
        for phase_modulation in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::from(i32::MIN) - 1.0,
            f64::from(i32::MAX) + 1.0,
        ] {
            let invalid = OscillatorInput {
                phase_modulation,
                ..input
            };
            assert!(!invalid.is_valid());
            assert_eq!(
                audio_sample(&mut state, &invalid, 4.0, &[[1.0; 2]; 7])[0],
                0.0
            );
        }
        for unison_voices in [0, 8, u32::MAX] {
            assert_eq!(
                audio_sample(
                    &mut state,
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
                audio_sample(
                    &mut state,
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
            assert_eq!(
                audio_sample(&mut state, &input, rate, &[[1.0; 2]; 7])[0],
                0.0
            );
        }
        assert!(
            (audio_sample(&mut state, &input, 4.0, &[[1.0; 2]; 7])[0]
                - std::f64::consts::FRAC_1_SQRT_2)
                .abs()
                < 1e-14
        );
        let mut extreme = OscillatorInput {
            waveform: Waveform::Sine,
            unison_voices: 7,
            detune_cents: 100.0,
            frequency_hz: f64::MAX,

            active: true,
            ..test_input()
        };
        for rate in [f64::MIN_POSITIVE, 44100.0, 96000.0, f64::MAX] {
            for frequency in [f64::MAX, 0.0] {
                extreme.frequency_hz = frequency;
                for _ in 0..16 {
                    assert!(
                        audio_sample(&mut state, &extreme, rate, &[[1.0; 2]; 7])[0].is_finite()
                    );
                }
            }
        }
    }
}

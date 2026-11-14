// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! TPT SVFの3出力と、その外側での出力選択・直列接続。
//! 数式の参考: <https://www.cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf>

/// VST3処理設定とフィルタで受理するサンプルレートの下限（Hz）。
pub const MIN_SAMPLE_RATE: f64 = 44_100.0;

/// SVFの外側で選ぶ応答と直列段数。各段に同じカットオフ・Qを適用する。
#[derive(Clone, Copy, Debug)]
pub enum FilterType {
    /// 入力をそのまま返し、両段の履歴を消去する。
    None,
    /// ローパス1段。高域の傾斜は-12 dB/oct。
    LowPass1,
    /// ローパス2段。高域の傾斜は-24 dB/oct。
    LowPass2,
    /// ハイパス1段。低域の傾斜は12 dB/oct。
    HighPass1,
    /// ハイパス2段。低域の傾斜は24 dB/oct。
    HighPass2,
    /// バンドパス1段。両側それぞれ6 dB/octで減衰する。
    BandPass1,
    /// バンドパス2段。両側それぞれ12 dB/octで減衰する。
    BandPass2,
}

/// 毎サンプル参照する設定。タイプ・ノート情報・積分器の履歴は含まない。
#[derive(Clone, Copy, PartialEq)]
pub struct FilterParameters {
    /// そのサンプルで要求する具体的な周波数（Hz）。有限値を受理する。
    /// BPFでは中心周波数。適正範囲へのクランプはSVF内部で行う。
    pub cutoff_hz: f64,
    /// そのサンプルで適用する各段のQ（0.5〜10）。BPFの中心利得もQとなる。
    pub resonance_q: f64,
}

impl FilterParameters {
    pub fn is_valid(&self) -> bool {
        self.cutoff_hz.is_finite() & (0.5..=10.0).contains(&self.resonance_q)
    }
}

/// 同一サンプル・同一の積分器状態から得た3種類の出力。
#[allow(dead_code)] // 単体SVFの入口で使用する。音源のFilterはステレオ出力を使う。
#[derive(Default)]
pub struct SvfOutput {
    /// 低域通過出力。
    pub low: f64,
    /// 帯域通過出力。中心利得を1に正規化しない。
    pub band: f64,
    /// 高域通過出力。
    pub high: f64,
}

/// 左右の独立した積分器から同時に得た3種類の出力。
struct StereoSvfOutput {
    /// 左右の低域通過出力。
    low: [f64; 2],
    /// 左右の帯域通過出力。
    band: [f64; 2],
    /// 左右の高域通過出力。
    high: [f64; 2],
}

/// 同じHz・Q・レートで共用できる係数。積分器の状態は含まない。
#[derive(Clone, Copy)]
struct SvfCoefficients {
    /// プリワープ後の積分係数。
    g: f64,
    /// HP式の積分器側の倍率（1/Q + g）。
    damping_plus_g: f64,
    /// HP式の分母（1 + g * (g + 1/Q)）の逆数。左右・両段で共用する。
    inverse_denominator: f64,
}

/// 1段の台形積分器の履歴。係数cacheは含まない。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
struct SvfStageState {
    /// [帯域通過側, 低域通過側]。設定変更時も保持する。
    integrators: [f64; 2],
}

/// 係数を受け取り、積分器を更新してlow・band・highを生成する。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
struct SvfStage {
    /// この段の積分器履歴。
    state: SvfStageState,
}

/// 同じ段の左右積分器を各2laneにまとめた履歴。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
struct StereoSvfStageState {
    /// [帯域通過側の左右, 低域通過側の左右]。左右間で履歴を混合しない。
    integrators: [[f64; 2]; 2],
}

/// 左右の積分器を同じ係数で処理し、3種類のステレオ出力を生成する。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
struct StereoSvfStage {
    /// この段の左右積分器履歴。
    state: StereoSvfStageState,
}

/// 同じHz・Q・レートで共用する最後の係数。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
struct SvfCoefficientCacheState {
    /// 検証済み正レートのbitを非ゼロ整数で保持し、Noneの判別領域を共有する。
    coefficients: Option<(FilterParameters, std::num::NonZeroU64, SvfCoefficients)>,
}

/// パラメーターとレートを照合して必要な場合だけ係数を再計算する。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
struct SvfCoefficientCache {
    /// 最後の要求と計算済み係数。
    state: SvfCoefficientCacheState,
}

/// 単体SVF用の積分器と係数cache。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
pub struct SvfState {
    /// 3出力を生成する1段の積分器。
    stage: SvfStage,
    /// 単体APIでも未変更の係数を再計算しない。
    cache: SvfCoefficientCache,
}

/// 単体SVFの入力検証、係数準備と積分器処理を担当する。
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
pub struct Svf {
    /// 単体処理用の積分器と係数。
    state: SvfState,
}

impl Svf {
    /// 3出力を生成してから次サンプル用の状態を更新する。確保・ロック・I/Oは行わない。
    /// 不正入力では状態を保持して無音、演算オーバーフローでは状態を消去して無音を返す。
    #[allow(dead_code)] // 単体SVFの入口。多段FilterはStageと係数cacheを直接所有する。
    #[inline]
    pub fn process_sample(
        &mut self,
        sample: f64,
        parameters: &FilterParameters,
        sample_rate: f64,
    ) -> SvfOutput {
        if !valid_input(sample, parameters, sample_rate) {
            return SvfOutput::default();
        }
        // 厳密に無入力・無履歴のときだけ省略する。微小な通常数の残響は打ち切らない。
        if sample == 0.0 && self.state.stage.state.integrators == [0.0; 2] {
            return SvfOutput::default();
        }
        let coefficients = self
            .state
            .cache
            .prepare_coefficients(parameters, sample_rate);
        self.state
            .stage
            .process_prepared_sample(sample, coefficients)
    }
}

impl SvfCoefficientCache {
    #[inline]
    fn prepare_coefficients(
        &mut self,
        parameters: &FilterParameters,
        sample_rate: f64,
    ) -> SvfCoefficients {
        if let Some((previous, rate, cached)) = self.state.coefficients
            && previous == *parameters
            && rate.get() == sample_rate.to_bits()
        {
            return cached;
        }
        // 入力設定を書き換えず、係数計算時だけ20〜20,000 Hzへクランプする。
        // レートは44.1 kHz以上なので、20 kHzは常にNyquist未満となる。
        let cutoff_hz = parameters.cutoff_hz.clamp(20.0, 20_000.0);
        // 要求Hzが変わっても、クランプ後のHzが同じならプリワープを再利用する。
        // Qも同じなら全係数を再利用し、次回の高速判定用に要求値だけ更新する。
        let g = match self.state.coefficients {
            Some((previous, rate, cached))
                if previous.cutoff_hz.clamp(20.0, 20_000.0) == cutoff_hz
                    && rate.get() == sample_rate.to_bits() =>
            {
                if previous.resonance_q == parameters.resonance_q {
                    self.state.coefficients = Some((
                        *parameters,
                        std::num::NonZeroU64::new(sample_rate.to_bits()).unwrap(),
                        cached,
                    ));
                    return cached;
                }
                cached.g
            }
            _ => (std::f64::consts::PI * (cutoff_hz / sample_rate)).tan(),
        };
        let damping = 1.0 / parameters.resonance_q;
        let coefficients = SvfCoefficients {
            g,
            damping_plus_g: damping + g,
            inverse_denominator: 1.0 / (1.0 + g * (g + damping)),
        };
        self.state.coefficients = Some((
            *parameters,
            std::num::NonZeroU64::new(sample_rate.to_bits()).unwrap(),
            coefficients,
        ));
        coefficients
    }
}

impl SvfStage {
    // 呼び出し側で検証済みの入力・係数を使い、この段の積分器だけを更新する。
    #[inline]
    fn process_prepared_sample(&mut self, sample: f64, coefficients: SvfCoefficients) -> SvfOutput {
        // 呼び出し元で全体の無入力・無履歴を判定済み。段ごとの重複判定はしない。
        let g = coefficients.g;
        let [s1, s2] = self.state.integrators;
        // HP = input - damping * BP - LPと2つの台形積分器を連立して解く。
        let high =
            (sample - coefficients.damping_plus_g * s1 - s2) * coefficients.inverse_denominator;
        let band = s1 + g * high;
        let low = s2 + g * band;
        let next = [2.0 * band - s1, 2.0 * low - s2];
        // 出力の非有限値もnextへ伝播するため、2つの積分器でまとめて検出する。
        if !(next[0].is_finite() & next[1].is_finite()) {
            self.state.integrators = [0.0; 2];
            return SvfOutput::default();
        }
        self.state.integrators = next;
        SvfOutput { low, band, high }
    }
}

impl StereoSvfStage {
    /// 左右独立の履歴へ同じ係数を適用し、3種類のステレオ出力を返す。
    #[inline(always)]
    fn process_prepared_sample(
        &mut self,
        samples: [f64; 2],
        coefficients: SvfCoefficients,
    ) -> StereoSvfOutput {
        // SAFETY: 対応先x86_64はSSE2を備える。読み書きは各2要素配列の範囲内。
        unsafe {
            use std::arch::x86_64::*;
            let s1 = _mm_loadu_pd(self.state.integrators[0].as_ptr());
            let s2 = _mm_loadu_pd(self.state.integrators[1].as_ptr());
            let sample = _mm_loadu_pd(samples.as_ptr());
            let g = _mm_set1_pd(coefficients.g);
            let high = _mm_mul_pd(
                _mm_sub_pd(
                    _mm_sub_pd(
                        sample,
                        _mm_mul_pd(_mm_set1_pd(coefficients.damping_plus_g), s1),
                    ),
                    s2,
                ),
                _mm_set1_pd(coefficients.inverse_denominator),
            );
            let band = _mm_add_pd(s1, _mm_mul_pd(g, high));
            let low = _mm_add_pd(s2, _mm_mul_pd(g, band));
            let two = _mm_set1_pd(2.0);
            let a = _mm_sub_pd(_mm_mul_pd(two, band), s1);
            let b = _mm_sub_pd(_mm_mul_pd(two, low), s2);
            let sign = _mm_set1_pd(-0.0);
            let max = _mm_set1_pd(f64::MAX);
            let finite = _mm_and_pd(
                _mm_cmple_pd(_mm_andnot_pd(sign, a), max),
                _mm_cmple_pd(_mm_andnot_pd(sign, b), max),
            );
            let a = _mm_and_pd(a, finite);
            let b = _mm_and_pd(b, finite);
            _mm_storeu_pd(self.state.integrators[0].as_mut_ptr(), a);
            _mm_storeu_pd(self.state.integrators[1].as_mut_ptr(), b);
            let mut output = StereoSvfOutput {
                low: [0.0; 2],
                band: [0.0; 2],
                high: [0.0; 2],
            };
            _mm_storeu_pd(output.low.as_mut_ptr(), _mm_and_pd(low, finite));
            _mm_storeu_pd(output.band.as_mut_ptr(), _mm_and_pd(band, finite));
            _mm_storeu_pd(output.high.as_mut_ptr(), _mm_and_pd(high, finite));
            output
        }
    }
}

fn valid_input(sample: f64, parameters: &FilterParameters, sample_rate: f64) -> bool {
    sample.is_finite()
        & parameters.is_valid()
        & sample_rate.is_finite()
        & (sample_rate >= MIN_SAMPLE_RATE)
}

/// SVFの外側で出力を選択し、必要な段数だけ直列接続するコンポーネント。
#[cfg_attr(test, derive(Clone))]
pub struct FilterState {
    /// 2段のステレオSVF。左右の履歴を独立させ、未使用の段は消去する。
    stages: [StereoSvfStage; 2],
    /// 全段・左右で共用する係数cache。
    cache: SvfCoefficientCache,
}

/// Filterの処理を担当し、専用のStateを所有するコンポーネント。
#[cfg_attr(test, derive(Clone))]
pub struct Filter {
    /// このコンポーネントの内部状態。
    state: FilterState,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            state: FilterState {
                stages: std::array::from_fn(|_| StereoSvfStage::default()),
                cache: SvfCoefficientCache::default(),
            },
        }
    }

    /// 処理停止・再初期化用。残響を即座に消去する。
    pub fn reset(&mut self) {
        for stage in &mut self.state.stages {
            stage.state.integrators = [[0.0; 2]; 2];
        }
    }

    #[inline(always)]
    pub fn process_sample(
        &mut self,
        samples: [f64; 2],
        filter_type: FilterType,
        parameters: &FilterParameters,
        sample_rate: f64,
    ) -> [f64; 2] {
        if matches!(filter_type, FilterType::None) {
            // 履歴を進める前には必ず共用cacheで係数を準備する。
            // 初回バイパスで両方を消去すれば、以降のゼロ書込みは不要。
            if self.state.cache.state.coefficients.is_some() {
                self.reset();
                self.state.cache.state.coefficients = None;
            }
            return samples;
        }
        if !(valid_input(samples[0], parameters, sample_rate) & samples[1].is_finite()) {
            return [0.0; 2];
        }
        let kind = filter_type as usize - 1;
        let count = 1 + (kind & 1);
        let output_index = kind / 2;
        let keep_second = 0_u64.wrapping_sub((count - 1) as u64);
        for integrators in &mut self.state.stages[1].state.integrators {
            for value in integrators {
                *value = f64::from_bits(value.to_bits() & keep_second);
            }
        }
        if samples == [0.0; 2]
            && self.state.stages[..count]
                .iter()
                .all(|s| s.state.integrators == [[0.0; 2]; 2])
        {
            return [0.0; 2];
        }
        // 左右・両段で係数を共用し、積分器の履歴だけ独立させる。
        let coefficients = self
            .state
            .cache
            .prepare_coefficients(parameters, sample_rate);
        match output_index {
            0 => self.process_selected_sample::<0>(samples, coefficients, count),
            1 => self.process_selected_sample::<1>(samples, coefficients, count),
            _ => self.process_selected_sample::<2>(samples, coefficients, count),
        }
    }

    #[inline(always)]
    fn process_selected_sample<const OUTPUT: usize>(
        &mut self,
        samples: [f64; 2],
        coefficients: SvfCoefficients,
        count: usize,
    ) -> [f64; 2] {
        let mut output = samples;
        for stage in &mut self.state.stages[..count] {
            let result = stage.process_prepared_sample(output, coefficients);
            output = [result.low, result.high, result.band][OUTPUT];
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{PI, TAU};

    // 単体テストの入力例。製品のUI設定の初期値は定義しない。
    const TEST_PARAMETERS: FilterParameters = FilterParameters {
        cutoff_hz: 20_000.0,
        resonance_q: std::f64::consts::FRAC_1_SQRT_2,
    };

    const TYPES: [FilterType; 6] = [
        FilterType::LowPass1,
        FilterType::LowPass2,
        FilterType::HighPass1,
        FilterType::HighPass2,
        FilterType::BandPass1,
        FilterType::BandPass2,
    ];

    #[test]
    fn stereo_channels_share_coefficients_but_not_history() {
        for kind in TYPES {
            let count = 1 + ((kind as usize - 1) & 1);
            let index = (kind as usize - 1) / 2;
            let mut filter = Filter::new();
            let mut reference: [[Svf; 2]; 2] =
                std::array::from_fn(|_| std::array::from_fn(|_| Svf::default()));
            for n in 0..128 {
                let parameters = FilterParameters {
                    cutoff_hz: 100.0 + n as f64 * 100.0,
                    resonance_q: 0.5 + (n % 7) as f64,
                };
                let samples = [
                    if n == 0 { 1.0 } else { 0.0 },
                    if n == 32 { -0.5 } else { 0.0 },
                ];
                let actual = filter.process_sample(samples, kind, &parameters, 48000.0);
                for ch in 0..2 {
                    let mut expected = samples[ch];
                    for stage in &mut reference[ch][..count] {
                        let out = stage.process_sample(expected, &parameters, 48000.0);
                        expected = [out.low, out.high, out.band][index];
                    }
                    assert!((actual[ch] - expected).abs() < 1e-12);
                }
                if n < 32 {
                    assert_eq!(actual[1], 0.0);
                }
            }
            filter.reset();
            assert_eq!(
                filter.process_sample([0.0; 2], kind, &TEST_PARAMETERS, 48000.0),
                [0.0; 2]
            );
        }
    }

    #[test]
    fn cached_coefficients_match_direct_calculation_during_modulation() {
        for kind in TYPES {
            let mut filter = Filter::new();
            let mut history = [[0.0; 2]; 2];
            let (mut error_energy, mut reference_energy) = (0.0, 0.0);
            let mut parameters = TEST_PARAMETERS;
            let mut rate = 48_000.0;
            for n in 0..256 {
                if n % 7 == 0 {
                    // 上下限の内側・境界・外側を往復し、同じ境界内での要求変更も含める。
                    parameters.cutoff_hz = [
                        -100.0, 0.0, 19.0, 20.0, 21.0, 1000.0, 19_999.0, 20_000.0, 20_001.0,
                        30_000.0, 40_000.0, 10_000.0,
                    ][(n / 7) % 12];
                }
                if n % 11 == 0 {
                    parameters.resonance_q = 0.5 + (n % 17) as f64 * 0.5;
                }
                if n % 13 == 0 {
                    rate = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                }
                let input = if n % 3 == 0 { 0.5 } else { -0.25 };
                let g = (PI * (parameters.cutoff_hz.clamp(20.0, 20_000.0) / rate).min(0.49)).tan();
                let damping = 1.0 / parameters.resonance_q;
                let count = if matches!(
                    kind,
                    FilterType::LowPass2 | FilterType::HighPass2 | FilterType::BandPass2
                ) {
                    2
                } else {
                    1
                };
                let mut expected = input;
                for state in &mut history[..count] {
                    let [s1, s2] = *state;
                    let high = (expected - (damping + g) * s1 - s2) / (1.0 + g * (g + damping));
                    let band = s1 + g * high;
                    let low = s2 + g * band;
                    *state = [2.0 * band - s1, 2.0 * low - s2];
                    expected = match kind {
                        FilterType::LowPass1 | FilterType::LowPass2 => low,
                        FilterType::BandPass1 | FilterType::BandPass2 => band,
                        _ => high,
                    };
                }
                let actual = filter.process_sample([input; 2], kind, &parameters, rate)[0];
                assert!(actual.is_finite());
                error_energy += (actual - expected).powi(2);
                reference_energy += expected.powi(2);
            }
            // 応答ごとの256サンプルで、直接除算の参照波形に対する誤差RMSを−80 dB以下とする。
            let relative_error = (error_energy / reference_energy).sqrt();
            assert!(
                relative_error <= 1e-4,
                "{kind:?}: relative RMS error={relative_error:e}"
            );
        }
    }

    #[test]
    fn silence_keeps_tails_and_clears_unused_stages() {
        let mut filter = Filter::new();
        // 第1段が無音でも、第2段に残った信号は処理する。
        filter.state.stages[1].state.integrators = [[1e-100, 0.0], [2e-100, 0.0]];
        assert_ne!(
            filter.process_sample([0.0; 2], FilterType::LowPass2, &TEST_PARAMETERS, 48_000.0)[0],
            0.0
        );
        assert_ne!(filter.state.stages[1].state.integrators, [[0.0; 2]; 2]);
        filter.process_sample([0.0; 2], FilterType::LowPass1, &TEST_PARAMETERS, 48_000.0);
        assert_eq!(filter.state.stages[1].state.integrators, [[0.0; 2]; 2]);
        assert_eq!(
            filter.process_sample([0.0; 2], FilterType::LowPass2, &TEST_PARAMETERS, 48_000.0)[0],
            0.0
        );
        assert_ne!(
            filter.process_sample([1.0; 2], FilterType::LowPass2, &TEST_PARAMETERS, 48_000.0)[0],
            0.0
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 長いインパルス応答のDFTは通常のRustテストで検証する。
    fn responses_match_analog_transfer_with_bilinear_warping() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for filter_type in TYPES {
                for q in [0.5, std::f64::consts::FRAC_1_SQRT_2, 4.0, 10.0] {
                    let parameters = FilterParameters {
                        cutoff_hz: 1000.0,
                        resonance_q: q,
                    };
                    let mut state = Filter::new();
                    let impulse: Vec<_> = (0..8192)
                        .map(|n| {
                            state.process_sample(
                                [if n == 0 { 1.0 } else { 0.0 }; 2],
                                filter_type,
                                &parameters,
                                rate,
                            )[0]
                        })
                        .collect();
                    for hz in [125.0, 500.0, 1000.0, 2000.0, 8000.0] {
                        let (mut real, mut imaginary) = (0.0, 0.0);
                        for (n, sample) in impulse.iter().enumerate() {
                            let angle = TAU * hz * n as f64 / rate;
                            real += sample * angle.cos();
                            imaginary -= sample * angle.sin();
                        }
                        // H(s)の分母はs²+s/Q+1。BLTの周波数写像から独立に期待利得を求める。
                        let r = (PI * hz / rate).tan() / (PI * parameters.cutoff_hz / rate).tan();
                        let numerator = match filter_type {
                            FilterType::LowPass1 | FilterType::LowPass2 => 1.0,
                            FilterType::HighPass1 | FilterType::HighPass2 => r * r,
                            FilterType::BandPass1 | FilterType::BandPass2 => r,
                            FilterType::None => unreachable!(),
                        };
                        let mut gain = numerator / (1.0 - r * r).hypot(r / q);
                        if matches!(
                            filter_type,
                            FilterType::LowPass2 | FilterType::HighPass2 | FilterType::BandPass2
                        ) {
                            gain *= gain;
                        }
                        assert!(
                            (real.hypot(imaginary) - gain).abs() < 1e-7,
                            "{filter_type:?}, rate={rate}, Q={q}, hz={hz}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn state_survives_parameter_changes_and_unused_stage_is_cleared() {
        let mut parameters = TEST_PARAMETERS;
        let mut filter_type = FilterType::LowPass1;
        let mut state = Filter::new();
        assert_eq!(
            state.process_sample([0.0; 2], filter_type, &parameters, 48_000.0)[0],
            0.0
        );
        state.process_sample([1.0; 2], filter_type, &parameters, 48_000.0);
        parameters.cutoff_hz = 1000.0;
        parameters.resonance_q = 10.0;
        filter_type = FilterType::BandPass2;
        assert_ne!(
            state.process_sample([0.0; 2], filter_type, &parameters, 48_000.0)[0],
            0.0
        );
        assert_ne!(state.state.stages[1].state.integrators, [[0.0; 2]; 2]);
        filter_type = FilterType::BandPass1;
        state.process_sample([0.0; 2], filter_type, &parameters, 48_000.0);
        assert_eq!(state.state.stages[1].state.integrators, [[0.0; 2]; 2]);
        state.reset();
        assert_eq!(
            state.process_sample([0.0; 2], filter_type, &parameters, 48_000.0)[0],
            0.0
        );
    }

    #[test]
    fn extremes_and_modulation_remain_finite() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for filter_type in TYPES {
                let mut state = Filter::new();
                for n in 0..256 {
                    let parameters = FilterParameters {
                        cutoff_hz: if n % 2 == 0 { 1.0 } else { 100_000.0 },
                        resonance_q: if n % 3 == 0 { 0.5 } else { 10.0 },
                    };
                    assert!(
                        state.process_sample(
                            [if n % 7 == 0 { 16.0 } else { 0.0 }; 2],
                            filter_type,
                            &parameters,
                            rate
                        )[0]
                        .is_finite()
                    );
                }
            }
        }
    }

    #[test]
    fn frequencies_are_clamped_inside_filter_without_changing_input() {
        for rate in [44_100.0_f64, 48_000.0, 96_000.0, 192_000.0] {
            for filter_type in TYPES {
                for requested in [
                    -f64::MAX,
                    -1.0,
                    0.0,
                    19.0,
                    20.0,
                    21.0,
                    1000.0,
                    19_999.0,
                    20_000.0,
                    20_001.0,
                    f64::MAX,
                ] {
                    let parameters = FilterParameters {
                        cutoff_hz: requested,
                        ..TEST_PARAMETERS
                    };
                    assert!(parameters.is_valid());
                    let boundary = FilterParameters {
                        cutoff_hz: requested.clamp(20.0, 20_000.0),
                        ..TEST_PARAMETERS
                    };
                    let mut limited = Filter::new();
                    let mut explicit = Filter::new();
                    for n in 0..16 {
                        let sample = if n == 0 { 1.0 } else { 0.0 };
                        let actual =
                            limited.process_sample([sample; 2], filter_type, &parameters, rate)[0];
                        let expected =
                            explicit.process_sample([sample; 2], filter_type, &boundary, rate)[0];
                        assert!(
                            actual.is_finite() && (actual - expected).abs() < 1e-12,
                            "{filter_type:?}, rate={rate}, requested={requested}, sample={n}"
                        );
                    }
                    assert_eq!(parameters.cutoff_hz, requested);
                }
            }
        }
    }

    #[test]
    fn invalid_values_do_not_poison_state() {
        let mut state = Svf::default();
        state.process_sample(1.0, &TEST_PARAMETERS, 48_000.0);
        let previous = state.state.stage.state.integrators;
        let silent = |output: SvfOutput| {
            assert_eq!([output.low, output.band, output.high], [0.0; 3]);
        };
        for rate in [1.0, 22_050.0, 32_000.0, MIN_SAMPLE_RATE.next_down()] {
            silent(state.process_sample(1.0, &TEST_PARAMETERS, rate));
            assert_eq!(state.state.stage.state.integrators, previous);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0] {
            silent(state.process_sample(1.0, &TEST_PARAMETERS, bad));
            silent(state.process_sample(
                1.0,
                &FilterParameters {
                    resonance_q: bad,
                    ..TEST_PARAMETERS
                },
                48_000.0,
            ));
            assert_eq!(state.state.stage.state.integrators, previous);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            silent(state.process_sample(bad, &TEST_PARAMETERS, 48_000.0));
            silent(state.process_sample(
                1.0,
                &FilterParameters {
                    cutoff_hz: bad,
                    ..TEST_PARAMETERS
                },
                48_000.0,
            ));
            assert_eq!(state.state.stage.state.integrators, previous);
        }
        for q in [0.499, 10.1] {
            assert!(
                !FilterParameters {
                    resonance_q: q,
                    ..TEST_PARAMETERS
                }
                .is_valid()
            );
        }
        // ノート周波数の上下オクターブ変換は呼び出し側で行う。固定Hz範囲で拒否しない。
        for hz in [-100.0, 0.0, 1.0, 100_000.0] {
            assert!(
                FilterParameters {
                    cutoff_hz: hz,
                    ..TEST_PARAMETERS
                }
                .is_valid()
            );
        }
        silent(state.process_sample(f64::MAX, &TEST_PARAMETERS, 48_000.0));
        assert_eq!(state.state.stage.state.integrators, [0.0; 2]);
        assert!(
            state
                .process_sample(1.0, &TEST_PARAMETERS, 48_000.0)
                .low
                .is_finite()
        );
    }

    #[test]
    fn svf_outputs_share_state_and_accept_samplewise_modulation() {
        let mut state = Svf::default();
        for n in 0..128 {
            let parameters = FilterParameters {
                cutoff_hz: 200.0 * 2.0_f64.powf(n as f64 / 32.0),
                resonance_q: 0.5 + 9.5 * n as f64 / 127.0,
            };
            let input = if n == 0 { 1.0 } else { 0.0 };
            let output = state.process_sample(input, &parameters, 48_000.0);
            // 回路の帰還式。各出力を得るために状態が複数回更新されていないことも確認する。
            assert!(
                (output.high + output.band / parameters.resonance_q + output.low - input).abs()
                    < 1e-12
            );
            assert_ne!(output.band, 0.0); // 係数変更で積分器をリセットしない。
        }
    }

    #[test]
    fn bypass_is_exact_and_drops_old_tail() {
        let mut state = Filter::new();
        state.process_sample([1.0; 2], FilterType::LowPass2, &TEST_PARAMETERS, 48_000.0);
        for sample in [
            -2.0,
            -0.0,
            0.0,
            0.25,
            3.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let output =
                state.process_sample([sample; 2], FilterType::None, &TEST_PARAMETERS, 48_000.0);
            assert_eq!(output.map(f64::to_bits), [sample.to_bits(); 2]);
        }
        assert_eq!(
            state.process_sample([0.0; 2], FilterType::LowPass2, &TEST_PARAMETERS, 48_000.0)[0],
            0.0
        );
        // Noneでは使用しない周波数・Q・レートで信号を遮断しない。
        assert_eq!(
            state.process_sample(
                [0.25; 2],
                FilterType::None,
                &FilterParameters {
                    cutoff_hz: f64::NAN,
                    resonance_q: f64::NAN
                },
                f64::NAN
            )[0],
            0.25
        );
    }
}

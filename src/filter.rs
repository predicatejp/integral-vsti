// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! TPT SVFの3出力と、その外側での出力選択・直列接続。
//! 数式の参考: <https://www.cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf>

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
#[derive(Clone, Copy)]
pub struct FilterParameters {
    /// そのサンプルで要求する具体的な周波数（Hz）。有限値を受理する。
    /// BPFでは中心周波数。適正範囲へのクランプはSVF内部で行う。
    pub cutoff_hz: f64,
    /// そのサンプルで適用する各段のQ（0.5〜10）。BPFの中心利得もQとなる。
    pub resonance_q: f64,
}

impl FilterParameters {
    pub fn is_valid(&self) -> bool {
        self.cutoff_hz.is_finite() && (0.5..=10.0).contains(&self.resonance_q)
    }
}

/// 同一サンプル・同一の積分器状態から得た3種類の出力。
#[derive(Default)]
pub struct SvfOutput {
    /// 低域通過出力。
    pub low: f64,
    /// 帯域通過出力。中心利得を1に正規化しない。
    pub band: f64,
    /// 高域通過出力。
    pub high: f64,
}

/// 1段のTPT SVF。フィルタタイプ・多段化・ノートを扱わない。
#[derive(Default)]
pub struct SvfState {
    /// 台形積分器の履歴。[帯域通過側, 低域通過側]。設定変更時も保持する。
    integrators: [f64; 2],
}

/// 3出力を生成してから次サンプル用の状態を更新する。確保・ロック・I/Oは行わない。
/// 不正入力では状態を保持して無音、演算オーバーフローでは状態を消去して無音を返す。
pub fn process_svf_sample(
    state: &mut SvfState,
    sample: f64,
    parameters: &FilterParameters,
    sample_rate: f64,
) -> SvfOutput {
    if !valid_input(sample, parameters, sample_rate) {
        return SvfOutput::default();
    }
    // 入力設定を書き換えず、係数計算時だけ20〜20,000 Hzへクランプする。
    // 極端に低いレートでもNyquist未満という上限を優先する。
    let cutoff_hz = parameters.cutoff_hz.clamp(20.0, 20_000.0);
    let g = (std::f64::consts::PI * (cutoff_hz / sample_rate).min(0.49)).tan();
    let damping = 1.0 / parameters.resonance_q;
    let [s1, s2] = state.integrators;
    // HP = input - damping * BP - LPと2つの台形積分器を連立して解く。
    let high = (sample - (damping + g) * s1 - s2) / (1.0 + g * (g + damping));
    let band = s1 + g * high;
    let low = s2 + g * band;
    let next = [2.0 * band - s1, 2.0 * low - s2];
    if [low, band, high]
        .iter()
        .chain(next.iter())
        .any(|v| !v.is_finite())
    {
        *state = SvfState::default();
        return SvfOutput::default();
    }
    state.integrators = next;
    SvfOutput { low, band, high }
}

fn valid_input(sample: f64, parameters: &FilterParameters, sample_rate: f64) -> bool {
    sample.is_finite() && parameters.is_valid() && sample_rate.is_finite() && sample_rate > 0.0
}

/// SVFの外側で出力を選択し、必要な段数だけ直列接続するコンポーネント。
pub struct FilterState {
    /// 独立した2段のSVF。未使用の段は消去し、再使用時に古い残響を出さない。
    stages: [SvfState; 2],
}

pub fn new_state() -> FilterState {
    FilterState {
        stages: [SvfState::default(), SvfState::default()],
    }
}

/// 処理停止・再初期化用。残響を即座に消去する。
pub fn reset(state: &mut FilterState) {
    *state = new_state();
}

pub fn process_sample(
    state: &mut FilterState,
    sample: f64,
    filter_type: FilterType,
    parameters: &FilterParameters,
    sample_rate: f64,
) -> f64 {
    if matches!(filter_type, FilterType::None) {
        reset(state);
        return sample;
    }
    if !valid_input(sample, parameters, sample_rate) {
        return 0.0;
    }
    let count = match filter_type {
        FilterType::LowPass2 | FilterType::HighPass2 | FilterType::BandPass2 => 2,
        _ => 1,
    };
    let mut sample = sample;
    for stage in &mut state.stages[..count] {
        let output = process_svf_sample(stage, sample, parameters, sample_rate);
        sample = match filter_type {
            FilterType::LowPass1 | FilterType::LowPass2 => output.low,
            FilterType::HighPass1 | FilterType::HighPass2 => output.high,
            FilterType::BandPass1 | FilterType::BandPass2 => output.band,
            FilterType::None => unreachable!(),
        };
    }
    if count == 1 {
        state.stages[1] = SvfState::default();
    }
    sample
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
    #[cfg_attr(miri, ignore)] // 長いインパルス応答のDFTは通常のRustテストで検証する。
    fn responses_match_analog_transfer_with_bilinear_warping() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for filter_type in TYPES {
                for q in [0.5, std::f64::consts::FRAC_1_SQRT_2, 4.0, 10.0] {
                    let parameters = FilterParameters {
                        cutoff_hz: 1000.0,
                        resonance_q: q,
                    };
                    let mut state = new_state();
                    let impulse: Vec<_> = (0..8192)
                        .map(|n| {
                            process_sample(
                                &mut state,
                                if n == 0 { 1.0 } else { 0.0 },
                                filter_type,
                                &parameters,
                                rate,
                            )
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
        let mut state = new_state();
        assert_eq!(
            process_sample(&mut state, 0.0, filter_type, &parameters, 48_000.0),
            0.0
        );
        process_sample(&mut state, 1.0, filter_type, &parameters, 48_000.0);
        parameters.cutoff_hz = 1000.0;
        parameters.resonance_q = 10.0;
        filter_type = FilterType::BandPass2;
        assert_ne!(
            process_sample(&mut state, 0.0, filter_type, &parameters, 48_000.0),
            0.0
        );
        assert_ne!(state.stages[1].integrators, [0.0; 2]);
        filter_type = FilterType::BandPass1;
        process_sample(&mut state, 0.0, filter_type, &parameters, 48_000.0);
        assert_eq!(state.stages[1].integrators, [0.0; 2]);
        reset(&mut state);
        assert_eq!(
            process_sample(&mut state, 0.0, filter_type, &parameters, 48_000.0),
            0.0
        );
    }

    #[test]
    fn extremes_and_modulation_remain_finite() {
        for rate in [1000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for filter_type in TYPES {
                let mut state = new_state();
                for n in 0..256 {
                    let parameters = FilterParameters {
                        cutoff_hz: if n % 2 == 0 { 1.0 } else { 100_000.0 },
                        resonance_q: if n % 3 == 0 { 0.5 } else { 10.0 },
                    };
                    assert!(
                        process_sample(
                            &mut state,
                            if n % 7 == 0 { 16.0 } else { 0.0 },
                            filter_type,
                            &parameters,
                            rate
                        )
                        .is_finite()
                    );
                }
            }
        }
    }

    #[test]
    fn frequencies_are_clamped_inside_filter_without_changing_input() {
        for rate in [1.0_f64, 1000.0, 22_050.0, 44_100.0, 96_000.0] {
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
                        cutoff_hz: requested.clamp(20.0, 20_000.0).min(rate * 0.49),
                        ..TEST_PARAMETERS
                    };
                    let mut limited = new_state();
                    let mut explicit = new_state();
                    for n in 0..16 {
                        let sample = if n == 0 { 1.0 } else { 0.0 };
                        let actual =
                            process_sample(&mut limited, sample, filter_type, &parameters, rate);
                        let expected =
                            process_sample(&mut explicit, sample, filter_type, &boundary, rate);
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
        let mut state = SvfState::default();
        process_svf_sample(&mut state, 1.0, &TEST_PARAMETERS, 48_000.0);
        let previous = state.integrators;
        let silent = |output: SvfOutput| {
            assert_eq!([output.low, output.band, output.high], [0.0; 3]);
        };
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0] {
            silent(process_svf_sample(&mut state, 1.0, &TEST_PARAMETERS, bad));
            silent(process_svf_sample(
                &mut state,
                1.0,
                &FilterParameters {
                    resonance_q: bad,
                    ..TEST_PARAMETERS
                },
                48_000.0,
            ));
            assert_eq!(state.integrators, previous);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            silent(process_svf_sample(
                &mut state,
                bad,
                &TEST_PARAMETERS,
                48_000.0,
            ));
            silent(process_svf_sample(
                &mut state,
                1.0,
                &FilterParameters {
                    cutoff_hz: bad,
                    ..TEST_PARAMETERS
                },
                48_000.0,
            ));
            assert_eq!(state.integrators, previous);
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
        silent(process_svf_sample(
            &mut state,
            f64::MAX,
            &TEST_PARAMETERS,
            48_000.0,
        ));
        assert_eq!(state.integrators, [0.0; 2]);
        assert!(
            process_svf_sample(&mut state, 1.0, &TEST_PARAMETERS, 48_000.0)
                .low
                .is_finite()
        );
    }

    #[test]
    fn svf_outputs_share_state_and_accept_samplewise_modulation() {
        let mut state = SvfState::default();
        for n in 0..128 {
            let parameters = FilterParameters {
                cutoff_hz: 200.0 * 2.0_f64.powf(n as f64 / 32.0),
                resonance_q: 0.5 + 9.5 * n as f64 / 127.0,
            };
            let input = if n == 0 { 1.0 } else { 0.0 };
            let output = process_svf_sample(&mut state, input, &parameters, 48_000.0);
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
        let mut state = new_state();
        process_sample(
            &mut state,
            1.0,
            FilterType::LowPass2,
            &TEST_PARAMETERS,
            48_000.0,
        );
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
            let output = process_sample(
                &mut state,
                sample,
                FilterType::None,
                &TEST_PARAMETERS,
                48_000.0,
            );
            assert_eq!(output.to_bits(), sample.to_bits());
        }
        assert_eq!(
            process_sample(
                &mut state,
                0.0,
                FilterType::LowPass2,
                &TEST_PARAMETERS,
                48_000.0
            ),
            0.0
        );
        // Noneでは使用しない周波数・Q・レートで信号を遮断しない。
        assert_eq!(
            process_sample(
                &mut state,
                0.25,
                FilterType::None,
                &FilterParameters {
                    cutoff_hz: f64::NAN,
                    resonance_q: f64::NAN
                },
                f64::NAN
            ),
            0.25
        );
    }
}

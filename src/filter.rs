// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 台形積分によるTPT SVF。設定と積分器の内部状態を分離する。
//! 数式の参考: https://www.cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf

/// 応答と直列段数。各段に同じカットオフ・Qを適用する。
#[derive(Clone, Copy, Debug)]
pub enum FilterType {
    /// ローパス1段。高域の傾斜は-12 dB/oct。
    LowPass12,
    /// ローパス2段。高域の傾斜は-24 dB/oct。
    LowPass24,
    /// ハイパス1段。低域の傾斜は12 dB/oct。
    HighPass12,
    /// ハイパス2段。低域の傾斜は24 dB/oct。
    HighPass24,
    /// バンドパス1段。両側それぞれ6 dB/octで減衰する。
    BandPass1,
    /// バンドパス2段。両側それぞれ12 dB/octで減衰する。
    BandPass2,
}

/// 毎サンプル参照するフィルタ設定。音声サンプルや積分器の履歴を含まない。
#[derive(Clone, Copy)]
pub struct FilterParameters {
    /// 応答と段数。変更時のクロスフェードは行わない。
    pub filter_type: FilterType,
    /// カットオフ（BPFでは中心周波数）、20〜20,000 Hz。
    /// 処理時の上限はsample_rateの0.49倍とし、Nyquist周波数を避ける。
    pub cutoff_hz: f64,
    /// 各段の共振の強さQ（0.707〜10）。BPFの中心周波数での利得もQとなる。
    pub resonance_q: f64,
}

pub const DEFAULT_FILTER: FilterParameters = FilterParameters {
    filter_type: FilterType::LowPass12,
    cutoff_hz: 20_000.0,
    resonance_q: 0.707,
};

impl FilterParameters {
    pub fn is_valid(&self) -> bool {
        (20.0..=20_000.0).contains(&self.cutoff_hz) && (0.707..=10.0).contains(&self.resonance_q)
    }
}

/// 1本の音声信号を処理する内部状態。確保・ロック・I/Oを行わない。
pub struct FilterState {
    /// 各段の2つの台形積分器の履歴。[段番号][帯域通過側/低域通過側]。
    /// 有効な段は設定変更時も保持し、未使用の2段目は消去する。
    integrators: [[f64; 2]; 2],
}

impl FilterState {
    pub fn new() -> Self {
        Self {
            integrators: [[0.0; 2]; 2],
        }
    }

    /// 処理停止・再初期化用。残響を即座に消去する。
    pub fn reset(&mut self) {
        self.integrators = [[0.0; 2]; 2];
    }

    /// 入力1サンプルから出力を生成し、次サンプル用の積分器状態を更新する。
    /// 不正な入力・設定・レートは無音を返して状態を保持する。
    /// 演算がオーバーフローした場合は状態を消去して無音を返す。
    pub fn process(&mut self, sample: f64, parameters: &FilterParameters, sample_rate: f64) -> f64 {
        if !sample.is_finite()
            || !parameters.is_valid()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
        {
            return 0.0;
        }
        let g = (std::f64::consts::PI * (parameters.cutoff_hz / sample_rate).min(0.49)).tan();
        let damping = 1.0 / parameters.resonance_q;
        let denominator = 1.0 + g * (g + damping);
        let stages = match parameters.filter_type {
            FilterType::LowPass12 | FilterType::HighPass12 | FilterType::BandPass1 => 1,
            FilterType::LowPass24 | FilterType::HighPass24 | FilterType::BandPass2 => 2,
        };
        let mut output = sample;
        for state in &mut self.integrators[..stages] {
            // HP = input - damping * BP - LPと2つの台形積分器を連立して解く。
            let high = (output - (damping + g) * state[0] - state[1]) / denominator;
            let band = state[0] + g * high;
            let low = state[1] + g * band;
            *state = [2.0 * band - state[0], 2.0 * low - state[1]];
            output = match parameters.filter_type {
                FilterType::LowPass12 | FilterType::LowPass24 => low,
                FilterType::HighPass12 | FilterType::HighPass24 => high,
                FilterType::BandPass1 | FilterType::BandPass2 => band,
            };
        }
        if stages == 1 {
            self.integrators[1] = [0.0; 2];
        }
        if !output.is_finite() || self.integrators.iter().flatten().any(|v| !v.is_finite()) {
            self.reset();
            return 0.0;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{PI, TAU};

    const TYPES: [FilterType; 6] = [
        FilterType::LowPass12,
        FilterType::LowPass24,
        FilterType::HighPass12,
        FilterType::HighPass24,
        FilterType::BandPass1,
        FilterType::BandPass2,
    ];

    #[test]
    #[cfg_attr(miri, ignore)] // 長いインパルス応答のDFTは通常のRustテストで検証する。
    fn responses_match_analog_transfer_with_bilinear_warping() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for filter_type in TYPES {
                for q in [0.707, 4.0, 10.0] {
                    let parameters = FilterParameters {
                        filter_type,
                        cutoff_hz: 1000.0,
                        resonance_q: q,
                    };
                    let mut state = FilterState::new();
                    let impulse: Vec<_> = (0..8192)
                        .map(|n| state.process(if n == 0 { 1.0 } else { 0.0 }, &parameters, rate))
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
                            FilterType::LowPass12 | FilterType::LowPass24 => 1.0,
                            FilterType::HighPass12 | FilterType::HighPass24 => r * r,
                            FilterType::BandPass1 | FilterType::BandPass2 => r,
                        };
                        let mut gain = numerator / (1.0 - r * r).hypot(r / q);
                        if matches!(
                            filter_type,
                            FilterType::LowPass24 | FilterType::HighPass24 | FilterType::BandPass2
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
        let mut parameters = DEFAULT_FILTER;
        let mut state = FilterState::new();
        assert_eq!(state.process(0.0, &parameters, 48_000.0), 0.0);
        state.process(1.0, &parameters, 48_000.0);
        parameters.cutoff_hz = 1000.0;
        parameters.resonance_q = 10.0;
        parameters.filter_type = FilterType::BandPass2;
        assert_ne!(state.process(0.0, &parameters, 48_000.0), 0.0);
        assert_ne!(state.integrators[1], [0.0; 2]);
        parameters.filter_type = FilterType::BandPass1;
        state.process(0.0, &parameters, 48_000.0);
        assert_eq!(state.integrators[1], [0.0; 2]);
        state.reset();
        assert_eq!(state.process(0.0, &parameters, 48_000.0), 0.0);
    }

    #[test]
    fn extremes_and_modulation_remain_finite() {
        for rate in [1000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for filter_type in TYPES {
                let mut state = FilterState::new();
                for n in 0..256 {
                    let parameters = FilterParameters {
                        filter_type,
                        cutoff_hz: if n % 2 == 0 { 20.0 } else { 20_000.0 },
                        resonance_q: if n % 3 == 0 { 0.707 } else { 10.0 },
                    };
                    assert!(
                        state
                            .process(if n % 7 == 0 { 16.0 } else { 0.0 }, &parameters, rate)
                            .is_finite()
                    );
                }
            }
        }
        // Nyquist未満への上限制限は、実際にその周波数を指定した場合と等しい。
        let mut limited = FilterState::new();
        let mut explicit = FilterState::new();
        for n in 0..32 {
            let sample = if n == 0 { 1.0 } else { 0.0 };
            assert_eq!(
                limited.process(sample, &DEFAULT_FILTER, 1000.0),
                explicit.process(
                    sample,
                    &FilterParameters {
                        cutoff_hz: 490.0,
                        ..DEFAULT_FILTER
                    },
                    1000.0
                )
            );
        }
    }

    #[test]
    fn invalid_values_do_not_poison_state() {
        let mut state = FilterState::new();
        state.process(1.0, &DEFAULT_FILTER, 48_000.0);
        let previous = state.integrators;
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0] {
            assert_eq!(state.process(1.0, &DEFAULT_FILTER, bad), 0.0);
            for parameters in [
                FilterParameters {
                    cutoff_hz: bad,
                    ..DEFAULT_FILTER
                },
                FilterParameters {
                    resonance_q: bad,
                    ..DEFAULT_FILTER
                },
            ] {
                assert_eq!(state.process(1.0, &parameters, 48_000.0), 0.0);
            }
            assert_eq!(state.integrators, previous);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(state.process(bad, &DEFAULT_FILTER, 48_000.0), 0.0);
            assert_eq!(state.integrators, previous);
        }
        assert!(
            !FilterParameters {
                cutoff_hz: 20_001.0,
                ..DEFAULT_FILTER
            }
            .is_valid()
        );
        assert!(
            !FilterParameters {
                resonance_q: 10.1,
                ..DEFAULT_FILTER
            }
            .is_valid()
        );
        state.process(f64::MAX, &DEFAULT_FILTER, 48_000.0);
        assert_eq!(state.integrators, [[0.0; 2]; 2]);
        assert!(state.process(1.0, &DEFAULT_FILTER, 48_000.0).is_finite());
    }
}

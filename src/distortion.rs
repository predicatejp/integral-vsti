// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! フィルタ出力のプリゲインとクリップ。ADAA履歴はVoice・左右で独立。

#[path = "distortion/soft.rs"]
mod soft;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipperType {
    None,
    Soft,
    Hard,
}

#[derive(Clone, Copy)]
pub struct DistortionParameters {
    /// NoneはGainとMixも含めたバイパス。
    pub clipper: ClipperType,
    /// 処理音のクリップ前に掛ける線形倍率。dBからの換算は設定側で行う。
    pub gain: f64,
    /// 処理音の混合率（0〜1）。原音側にはGainを掛けない。
    pub mix: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaaOrder {
    None,
    First,
    Second,
}

struct DistortionState {
    /// 過去2点のGain適用済み入力。Gain変更時も当時の値を維持する。
    input: [[f64; 2]; 2],
    /// 過去2点の原始関数。1次はF1、2次はF2を保持する。
    integral: [[f64; 2]; 2],
}

pub struct Distortion {
    state: DistortionState,
}
impl Distortion {
    pub fn new() -> Self {
        Self {
            state: DistortionState {
                input: [[0.0; 2]; 2],
                integral: [[0.0; 2]; 2],
            },
        }
    }
    pub fn reset(&mut self) {
        self.state.input = [[0.0; 2]; 2];
        self.state.integral = [[0.0; 2]; 2];
    }
    /// Wetだけを出力する。DryとMixはホストレートで独立して処理する。
    #[inline]
    pub fn process_sample(
        &mut self,
        input: [f64; 2],
        parameters: &DistortionParameters,
        order: AdaaOrder,
    ) -> [f64; 2] {
        if parameters.clipper == ClipperType::None {
            return input;
        }
        if order == AdaaOrder::None {
            let driven = input.map(|x| x * parameters.gain);
            return if parameters.clipper == ClipperType::Soft {
                driven.map(f64::tanh)
            } else {
                driven.map(|x| x.clamp(-1.0, 1.0))
            };
        }
        match (
            parameters.clipper == ClipperType::Soft,
            order == AdaaOrder::Second,
        ) {
            (true, false) => self.process_adaa_sample::<true, false>(input, parameters.gain),
            (true, true) => self.process_adaa_sample::<true, true>(input, parameters.gain),
            (false, false) => self.process_adaa_sample::<false, false>(input, parameters.gain),
            (false, true) => self.process_adaa_sample::<false, true>(input, parameters.gain),
        }
    }

    // ADAAの大きな計算経路を分離し、Noneの経路にレジスター退避を持ち込まない。
    #[inline(never)]
    fn process_adaa_sample<const SOFT: bool, const SECOND: bool>(
        &mut self,
        input: [f64; 2],
        gain: f64,
    ) -> [f64; 2] {
        std::array::from_fn(|ch| {
            let x = input[ch] * gain;
            let previous = self.state.input[0][ch];
            let values = if SOFT { soft::eval(x) } else { [0.0; 4] };
            let integral = if !SECOND { values[1] } else { values[0] };
            let out = if !SECOND {
                if !SOFT {
                    hard_first(x, previous)
                } else if (x - previous).abs() > 1e-6 * (1.0 + x.abs().max(previous.abs())) {
                    (integral - self.state.integral[0][ch]) / (x - previous)
                } else {
                    soft::eval((x + previous) * 0.5)[2]
                }
            } else {
                // 第2差商は入力の置換に対称。外側の分母を最大幅にして桁落ちを抑える。
                let mut v = [
                    (x, integral),
                    (previous, self.state.integral[0][ch]),
                    (self.state.input[1][ch], self.state.integral[1][ch]),
                ];
                if v[0].0 > v[1].0 {
                    v.swap(0, 1);
                }
                if v[1].0 > v[2].0 {
                    v.swap(1, 2);
                }
                if v[0].0 > v[1].0 {
                    v.swap(0, 1);
                }
                let [(a, ga), (b, gb), (c, gc)] = v;
                if !SOFT {
                    hard_second(a, b, c)
                } else {
                    soft::second(a, b, c, ga, gb, gc)
                }
            };
            self.state.input[1][ch] = previous;
            self.state.input[0][ch] = x;
            self.state.integral[1][ch] = self.state.integral[0][ch];
            self.state.integral[0][ch] = integral;
            out
        })
    }
}

fn hard_first(x: f64, previous: f64) -> f64 {
    let a = x.min(previous);
    let b = x.max(previous);
    if a >= 1. {
        1.
    } else if b <= -1. {
        -1.
    } else if a >= -1. && b <= 1. {
        (a + b) * 0.5
    } else {
        let lo = a.max(-1.);
        let hi = b.min(1.);
        ((lo + hi) * (hi - lo) * 0.5 + (b - hi) - (lo - a)) / (b - a)
    }
}

// 区分的な一次クリップ関数を三角形重みで厳密積分する。
fn hard_second(a: f64, b: f64, c: f64) -> f64 {
    if a >= 1. {
        return 1.;
    }
    if c <= -1. {
        return -1.;
    }
    if a >= -1. && c <= 1. {
        return (a + b + c) / 3.;
    }
    let span = c - a;
    let integrate = |lo: f64, hi: f64, rising: bool| {
        if hi == lo {
            return 0.;
        }
        let mut sum = 0.;
        let mut l = lo;
        for r in [(-1.0_f64).clamp(lo, hi), 1.0_f64.clamp(lo, hi), hi] {
            let d = r - l;
            if d > 0. {
                let mid = l + d * 0.5;
                let val = mid.clamp(-1., 1.);
                let slope = if l >= -1. && r <= 1. { 1. } else { 0. };
                let (weight, ws) = if rising {
                    (((l - lo) + d * 0.5) / (hi - lo), 1. / (hi - lo))
                } else {
                    (((hi - r) + d * 0.5) / (hi - lo), -1. / (hi - lo))
                };
                sum += d * (val * weight + slope * ws * d * d / 12.);
            }
            l = r;
        }
        sum
    };
    2. * (integrate(a, b, true) + integrate(b, c, false)) / span
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_response_gain_history_and_reset() {
        for order in [AdaaOrder::None, AdaaOrder::First, AdaaOrder::Second] {
            let mut d = Distortion::new();
            let mut history = [0.0; 2];
            for n in 0..500 {
                let x = ((n * 37 % 101) as f64 - 50.0) / 10000.0;
                let gain = if n % 7 == 0 { 20.0 } else { 1.0 };
                let p = DistortionParameters {
                    clipper: ClipperType::Hard,
                    gain,
                    mix: 1.0,
                };
                let current = x * gain;
                let expected = match order {
                    AdaaOrder::None => current,
                    AdaaOrder::First => (current + history[0]) * 0.5,
                    AdaaOrder::Second => (current + history[0] + history[1]) / 3.0,
                };
                let actual = d.process_sample([x, -0.3 * x], &p, order);
                assert!((actual[0] - expected).abs() < 1e-15);
                assert!((actual[1] + expected * 0.3).abs() < 1e-15);
                history = [current, history[0]];
            }
            d.reset();
            let p = DistortionParameters {
                clipper: ClipperType::Soft,
                gain: 1.0,
                mix: 1.0,
            };
            assert_eq!(d.process_sample([0.0; 2], &p, order), [0.0; 2]);
        }
    }
    #[test]
    fn direct_and_constant_inputs_match_the_clipper() {
        for order in [AdaaOrder::None, AdaaOrder::First, AdaaOrder::Second] {
            for clipper in [ClipperType::None, ClipperType::Soft, ClipperType::Hard] {
                for x in [
                    -1e6_f64, -16.0, -1.0, -0.001, 0.0, 0.002, 0.5, 1.0, 16.0, 1e6,
                ] {
                    let mut d = Distortion::new();
                    let p = DistortionParameters {
                        clipper,
                        gain: 2.0,
                        mix: 1.0,
                    };
                    let expected = match clipper {
                        ClipperType::None => x,
                        ClipperType::Soft => (x * 2.0).tanh(),
                        ClipperType::Hard => (x * 2.0).clamp(-1.0, 1.0),
                    };
                    d.process_sample([x, -x], &p, order);
                    d.process_sample([x, -x], &p, order);
                    let out = d.process_sample([x, -x], &p, order);
                    assert!(
                        (out[0] - expected).abs() < 7e-8,
                        "{order:?} {clipper:?} {x}: {out:?}"
                    );
                    assert!((out[1] + expected).abs() < 7e-8);
                }
            }
        }
    }
    #[test]
    fn second_order_near_equal_and_crossing_inputs_match_high_precision_reference() {
        // 70桁の原始関数による差商・重複点の極限値から求めた固定参照。
        let cases: [([f64; 3], f64, f64); 10] = [
            (
                [0.0, 0.0, 0.01],
                0.0033333000006349057,
                0.0033333333333333335,
            ),
            (
                [1.0, 1.000000000001, 0.999999999999],
                0.7615941559557649,
                0.9999999999998334,
            ),
            ([15.999999, 16.0, 16.000001], 0.9999999999999747, 1.0),
            ([-2.0, 0.5, 2.0], 0.12459722909143241, 0.14444444444444443),
            ([-1000000.0, 0.0, 1000000.0], 0.0, 0.0),
            (
                [
                    0.0033012619744718303,
                    0.0029476632258353752,
                    0.0032967554184158265,
                ],
                0.003181882779597484,
                0.003181893539574344,
            ),
            ([0.001, -0.001, 0.0], 0.0, 0.0),
            ([-3.0, -3.0, 2.0], -0.6160212980525002, -0.6533333333333333),
            (
                [0.1, 0.100000000001, 0.099999999999],
                0.09966799462495582,
                0.1,
            ),
            ([1.0, 1.0, -1.0], 0.2812007366881278, 0.3333333333333333),
        ];
        for (input, soft, hard) in cases {
            for (clipper, expected, tolerance) in [
                (ClipperType::Soft, soft, 7e-8),
                (ClipperType::Hard, hard, 1e-14),
            ] {
                let mut d = Distortion::new();
                let p = DistortionParameters {
                    clipper,
                    gain: 1.0,
                    mix: 1.0,
                };
                let mut out = [0.0; 2];
                for x in input {
                    out = d.process_sample([x, -x], &p, AdaaOrder::Second);
                }
                assert!(
                    (out[0] - expected).abs() < tolerance,
                    "{clipper:?} {input:?}: {out:?} {expected}"
                );
                assert!((out[1] + expected).abs() < tolerance);
            }
        }
    }
}

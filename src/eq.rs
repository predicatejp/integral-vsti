// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! TPT SVFの出力混合によるLow shelf・Peak・High shelfの3帯域EQ。

use std::f64::consts::{PI, SQRT_2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqParameters {
    /// Offでは入力をそのまま返す。
    pub enabled: bool,
    /// Low/Mid/Highの基準周波数（Hz）。
    pub frequencies_hz: [f64; 3],
    /// 各帯域の増減量（dB）。
    pub gains_db: [f64; 3],
    /// MidのQ。棚型の傾斜は固定。
    pub mid_q: f64,
}

#[cfg_attr(test, derive(Clone))]
struct EqState {
    /// 各帯域・左右の2積分器状態。
    integrators: [[[f64; 2]; 2]; 3],
    /// 各帯域の[a1,a2,a3,m0,m1,m2]。設定変更時だけ設計する。
    coefficients: [[f64; 6]; 3],
    /// 適用中の設定とレート。
    parameters: EqParameters,
    /// 準備済みのサンプルレート（Hz）。
    sample_rate: f64,
}

#[cfg_attr(test, derive(Clone))]
pub struct Eq {
    /// 専用の積分器履歴・係数・設定。
    state: EqState,
}

impl Eq {
    pub fn new(sample_rate: f64, parameters: EqParameters) -> Self {
        let mut result = Self {
            state: EqState {
                integrators: [[[0.; 2]; 2]; 3],
                coefficients: [[0.; 6]; 3],
                parameters,
                sample_rate,
            },
        };
        for i in 0..3 {
            result.update_coefficients(i);
        }
        result
    }

    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        if sample_rate != self.state.sample_rate {
            self.state.sample_rate = sample_rate;
            for i in 0..3 {
                self.update_coefficients(i);
            }
            self.reset();
        }
        true
    }

    pub fn enabled(&self) -> bool {
        self.state.parameters.enabled
    }

    pub fn set_parameters(&mut self, parameters: EqParameters) {
        let old = self.state.parameters;
        self.state.parameters = parameters;
        for i in 0..3 {
            if old.frequencies_hz[i] != parameters.frequencies_hz[i]
                || old.gains_db[i] != parameters.gains_db[i]
                || (i == 1 && old.mid_q != parameters.mid_q)
            {
                self.update_coefficients(i);
            }
        }
        if old.enabled != parameters.enabled {
            self.reset();
        }
    }

    fn update_coefficients(&mut self, band: usize) {
        let p = self.state.parameters;
        let a = 10_f64.powf(p.gains_db[band] / 40.);
        let mut g = (PI * p.frequencies_hz[band].min(self.state.sample_rate * 0.49)
            / self.state.sample_rate)
            .tan();
        // 固定時のShelf/Peak応答。状態は台形積分器の共通座標で保持する。
        let (k, m0, m1, m2) = match band {
            0 => {
                g /= a.sqrt();
                (SQRT_2, 1., SQRT_2 * (a - 1.), a * a - 1.)
            }
            1 => {
                let k = 1. / (p.mid_q * a);
                (k, 1., k * (a * a - 1.), 0.)
            }
            _ => {
                g *= a.sqrt();
                (SQRT_2, a * a, SQRT_2 * (1. - a) * a, 1. - a * a)
            }
        };
        let a1 = 1. / (1. + g * (g + k));
        self.state.coefficients[band] = [a1, g * a1, g * g * a1, m0, m1, m2];
    }

    pub fn reset(&mut self) {
        self.state.integrators = [[[0.; 2]; 2]; 3];
    }

    pub fn process_sample(&mut self, mut input: [f64; 2]) -> [f64; 2] {
        if !self.enabled() {
            return input;
        }
        for band in 0..3 {
            let [a1, a2, a3, m0, m1, m2] = self.state.coefficients[band];
            for (x, state) in input.iter_mut().zip(&mut self.state.integrators[band]) {
                let v3 = *x - state[1];
                let v1 = a1 * state[0] + a2 * v3;
                let v2 = state[1] + a2 * state[0] + a3 * v3;
                state[0] = 2. * v1 - state[0];
                state[1] = 2. * v2 - state[1];
                *x = m0 * *x + m1 * v1 + m2 * v2;
            }
        }
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> EqParameters {
        EqParameters {
            enabled: true,
            frequencies_hz: [250., 1000., 4000.],
            gains_db: [0.; 3],
            mid_q: 1.,
        }
    }
    #[test]
    fn flat_and_bypass_are_unity_and_reset_discards_history() {
        let mut p = settings();
        let mut eq = Eq::new(48000., p);
        for i in 0..1000 {
            let input = [(i as f64 * 0.2).sin(), (i as f64 * 0.3).cos()];
            assert_eq!(eq.process_sample(input), input);
        }
        p.enabled = false;
        p.gains_db = [18.; 3];
        eq.set_parameters(p);
        assert_eq!(eq.process_sample([0.1, -0.3]), [0.1, -0.3]);
        p.enabled = true;
        eq.set_parameters(p);
        eq.reset();
        for _ in 0..1000 {
            assert_eq!(eq.process_sample([0.; 2]), [0.; 2]);
        }
    }
    #[test]
    fn peak_center_gain_and_shelf_limits_match_their_settings() {
        for rate in [44100., 48000., 96000.] {
            for gain in [-18., -6., 6., 18.] {
                let mut p = settings();
                p.gains_db[1] = gain;
                let mut eq = Eq::new(rate, p);
                let mut input_energy = 0.;
                let mut output_energy = 0.;
                for i in 0..rate as usize {
                    let x = (2. * PI * 1000. * i as f64 / rate).sin();
                    let y = eq.process_sample([x; 2]);
                    if i > rate as usize / 2 {
                        input_energy += x * x;
                        output_energy += y[0] * y[0];
                    }
                }
                assert!((10. * (output_energy / input_energy).log10() - gain).abs() < 0.001);
                p.gains_db = [gain, 0., -gain];
                eq.set_parameters(p);
                eq.reset();
                let mut dc = [0.; 2];
                for _ in 0..rate as usize {
                    dc = eq.process_sample([1.; 2]);
                }
                assert!((dc[0] - 10_f64.powf(gain / 20.)).abs() < 1e-8);
            }
        }
    }
    #[test]
    fn rapid_parameter_changes_remain_finite() {
        let mut p = settings();
        let mut eq = Eq::new(44100., p);
        for i in 0..100000 {
            if i % 7 == 0 {
                let upper = i % 14 == 0;
                p.frequencies_hz = if upper { [20000.; 3] } else { [20.; 3] };
                p.gains_db = if upper { [18.; 3] } else { [-18.; 3] };
                p.mid_q = if upper { 10. } else { 0.1 };
                eq.set_parameters(p);
            }
            let out = eq.process_sample(if i < 10 { [1., -1.] } else { [0.; 2] });
            assert!(out.into_iter().all(|v| v.is_finite() && v.abs() < 1000.));
        }
    }
}

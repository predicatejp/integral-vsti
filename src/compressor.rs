// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 左右の最大振幅を検出し、共通の減衰dBを平滑化するコンプレッサー。

#[derive(Clone, Copy, PartialEq)]
pub struct CompressorParameters {
    /// Offでは音声と時間位置をそのまま返す。
    pub enabled: bool,
    /// 圧縮を開始するレベル（dBFS）。
    pub threshold_db: f64,
    /// thresholdを超えた入力変化量と出力変化量の比。
    pub ratio: f64,
    /// 減衰dBの増加が目標差の63.2%へ到達する時間（ms）。
    pub attack_ms: f64,
    /// 減衰dBの回復が目標差の63.2%へ到達する時間（ms）。
    pub release_ms: f64,
    /// thresholdを中心とする二次soft kneeの幅（dB）。
    pub knee_db: f64,
    /// 処理音へ加える音量（dB）。
    pub makeup_db: f64,
    /// 原音0〜処理音1の線形混合量。
    pub mix: f64,
}

#[cfg_attr(test, derive(Clone))]
struct CompressorState {
    /// 適用中の操作値。係数は対応する値が変わったときだけ計算する。
    parameters: CompressorParameters,
    /// 時間係数を計算したサンプルレート。
    sample_rate: f64,
    /// 現在の減衰量（正のdB）。左右で共有する。
    reduction_db: f64,
    /// attack/releaseの1 sample残留係数。
    coefficients: [f64; 2],
    /// Makeupの線形振幅倍率。
    makeup_gain: f64,
    /// knee下端の線形振幅。無圧縮域では対数計算を省く。
    lower_gain: f64,
}

#[cfg_attr(test, derive(Clone))]
pub struct Compressor {
    /// 減衰履歴と設定から求めた係数。
    state: CompressorState,
}

impl Compressor {
    pub fn new(parameters: CompressorParameters, sample_rate: f64) -> Self {
        Self {
            state: CompressorState {
                parameters,
                sample_rate,
                reduction_db: 0.0,
                coefficients: [parameters.attack_ms, parameters.release_ms]
                    .map(|ms| coefficient(ms, sample_rate)),
                makeup_gain: 10.0_f64.powf(parameters.makeup_db / 20.0),
                lower_gain: 10.0_f64
                    .powf((parameters.threshold_db - parameters.knee_db * 0.5) / 20.0),
            },
        }
    }

    pub fn set_parameters(&mut self, parameters: CompressorParameters, sample_rate: f64) {
        let old = self.state.parameters;
        if parameters.attack_ms != old.attack_ms || sample_rate != self.state.sample_rate {
            self.state.coefficients[0] = coefficient(parameters.attack_ms, sample_rate);
        }
        if parameters.release_ms != old.release_ms || sample_rate != self.state.sample_rate {
            self.state.coefficients[1] = coefficient(parameters.release_ms, sample_rate);
        }
        if parameters.makeup_db != old.makeup_db {
            self.state.makeup_gain = 10.0_f64.powf(parameters.makeup_db / 20.0);
        }
        if parameters.threshold_db != old.threshold_db || parameters.knee_db != old.knee_db {
            self.state.lower_gain =
                10.0_f64.powf((parameters.threshold_db - parameters.knee_db * 0.5) / 20.0);
        }
        if parameters.enabled != old.enabled {
            self.reset();
        }
        self.state.parameters = parameters;
        self.state.sample_rate = sample_rate;
    }

    pub fn reset(&mut self) {
        self.state.reduction_db = 0.0;
    }

    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        let p = self.state.parameters;
        if !p.enabled {
            return input;
        }
        let peak = input[0].abs().max(input[1].abs());
        let target = if peak <= self.state.lower_gain {
            0.0
        } else {
            let over = 20.0 * peak.log10() - p.threshold_db;
            let curve = if p.knee_db > 0.0 && over < p.knee_db * 0.5 {
                (over + p.knee_db * 0.5).powi(2) / (2.0 * p.knee_db)
            } else {
                over
            };
            curve * (1.0 - 1.0 / p.ratio)
        };
        let coefficient = self.state.coefficients[usize::from(target <= self.state.reduction_db)];
        self.state.reduction_db = target + coefficient * (self.state.reduction_db - target);
        let gain = 10.0_f64.powf(-self.state.reduction_db / 20.0) * self.state.makeup_gain;
        let mixed_gain = (1.0 - p.mix) + gain * p.mix;
        input.map(|sample| sample * mixed_gain)
    }
}

fn coefficient(ms: f64, sample_rate: f64) -> f64 {
    if ms == 0.0 {
        0.0
    } else {
        (-1000.0 / (ms * sample_rate)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters() -> CompressorParameters {
        CompressorParameters {
            enabled: true,
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 0.0,
            release_ms: 0.0,
            knee_db: 0.0,
            makeup_db: 0.0,
            mix: 1.0,
        }
    }

    #[test]
    fn static_curve_and_stereo_link_follow_the_requested_ratio() {
        let mut c = Compressor::new(parameters(), 48000.0);
        let y = c.process_sample([1.0, -0.25]);
        assert!((20.0 * y[0].log10() + 13.5).abs() < 1e-10);
        assert_eq!(y[1], y[0] * -0.25);
        let mut p = parameters();
        p.knee_db = 6.0;
        c.set_parameters(p, 48000.0);
        let x = 10.0_f64.powf(-18.0 / 20.0);
        let y = c.process_sample([x, x])[0];
        assert!((20.0 * (y / x).log10() + 0.5625).abs() < 1e-10);
    }

    #[test]
    fn time_constants_bypass_mix_and_reset_have_distinct_contracts() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let mut p = parameters();
            p.attack_ms = 10.0;
            p.release_ms = 100.0;
            let mut c = Compressor::new(p, rate);
            for _ in 0..(rate * 0.01) as usize {
                c.process_sample([1.0, 0.0]);
            }
            assert!((c.state.reduction_db / 13.5 - (1.0 - (-1.0_f64).exp())).abs() < 1e-10);
            let previous = c.state.reduction_db;
            for _ in 0..(rate * 0.1) as usize {
                assert_eq!(c.process_sample([0.0; 2]), [0.0; 2]);
            }
            assert!((c.state.reduction_db / previous - (-1.0_f64).exp()).abs() < 1e-10);
            p.mix = 0.0;
            c.set_parameters(p, rate);
            assert_eq!(c.process_sample([0.7, -0.2]), [0.7, -0.2]);
            p.enabled = false;
            c.set_parameters(p, rate);
            assert_eq!(c.process_sample([2.0, -3.0]), [2.0, -3.0]);
            assert_eq!(c.state.reduction_db, 0.0);
        }
    }
}

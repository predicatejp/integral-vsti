// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 6段の正規化ラティスallpassと遅延フィードバックによるPhaser。

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaserParameters {
    /// 各allpass段の−90°周波数（100〜4000 Hz）。
    pub center_hz: f64,
    /// 中心から上下への変調幅（0〜2400 cent）。
    pub depth_cents: f64,
    /// 左右共通LFO（0.05〜5 Hz）。
    pub rate_hz: f64,
    /// 直前のWetを入力へ戻す倍率（−0.95〜0.95）。
    pub feedback: f64,
    /// 原音0、処理音1の線形混合率。
    pub mix: f64,
}

#[cfg_attr(test, derive(Clone))]
struct PhaserState {
    /// 各段・左右独立の正規化ラティス状態。
    stages: [[f64; 2]; 6],
    /// フィードバック用の1sample前の出力。
    previous: [f64; 2],
    /// ホストのsample rate（Hz）。
    sample_rate: f64,
    /// 現在の定常設定。
    parameters: PhaserParameters,
    /// 現在・目標のallpass係数(a,b)。端点はa²+b²=1。
    coefficient: [f64; 2],
    /// 今の補間区間が到達する端点。
    target: [f64; 2],
    /// 目標へ64sampleで到達する増分。
    step: [f64; 2],
    /// 次の目標計算までの残りsample。
    remaining: u32,
    /// f64 MCFの正弦とずれた余弦。
    lfo: [f64; 2],
    /// MCFの回転係数2*sin(pi*rate/Fs)。
    epsilon: f64,
    /// epsilon/2とsqrt(1−(epsilon/2)^2)。
    half: [f64; 2],
}

#[cfg_attr(test, derive(Clone))]
pub struct Phaser {
    state: PhaserState,
}

impl Phaser {
    pub fn new(parameters: PhaserParameters) -> Self {
        let mut result = Self {
            state: PhaserState {
                stages: [[0.0; 2]; 6],
                previous: [0.0; 2],
                sample_rate: 44100.0,
                parameters,
                coefficient: [0.0; 2],
                target: [0.0; 2],
                step: [0.0; 2],
                remaining: 0,
                lfo: [0.0, 1.0],
                epsilon: 0.0,
                half: [0.0, 1.0],
            },
        };
        result.update_rate();
        result.reset();
        result
    }
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        if !sample_rate.is_finite() || sample_rate < 44100.0 {
            return false;
        }
        self.state.sample_rate = sample_rate;
        self.update_rate();
        self.reset();
        true
    }
    pub fn set_parameters(&mut self, p: PhaserParameters) {
        let old = self.state.parameters;
        self.state.parameters = p;
        if p.rate_hz != old.rate_hz {
            self.update_rate();
        }
    }
    fn update_rate(&mut self) {
        let s = &mut self.state;
        let cosine = (s.lfo[1] + s.lfo[0] * s.half[0]) / s.half[1];
        let h = (std::f64::consts::PI * s.parameters.rate_hz / s.sample_rate).sin();
        let g = (1.0 - h * h).sqrt();
        s.epsilon = 2.0 * h;
        s.half = [h, g];
        s.lfo[1] = cosine * g - s.lfo[0] * h;
    }
    fn coefficients(&self) -> [f64; 2] {
        let s = &self.state;
        let frequency =
            s.parameters.center_hz * (s.parameters.depth_cents * s.lfo[0] / 1200.0).exp2();
        // 検証済み設定とFs>=44.1kHzより、周波数は25〜16000Hz。
        let t = (std::f64::consts::PI * frequency / s.sample_rate).tan();
        [(t - 1.0) / (t + 1.0), 2.0 * t.sqrt() / (t + 1.0)]
    }
    pub fn reset(&mut self) {
        self.state.stages = [[0.0; 2]; 6];
        self.state.previous = [0.0; 2];
        self.state.lfo = [0.0, self.state.half[1]];
        self.state.coefficient = self.coefficients();
        self.state.target = self.state.coefficient;
        self.state.step = [0.0; 2];
        self.state.remaining = 0;
    }
    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        if self.state.remaining == 0 {
            self.state.target = self.coefficients();
            self.state.step =
                std::array::from_fn(|i| (self.state.target[i] - self.state.coefficient[i]) / 64.0);
            self.state.remaining = 64;
        }
        let s = &mut self.state;
        let [a, b] = s.coefficient;
        let mut wet = std::array::from_fn(|ch| input[ch] + s.previous[ch] * s.parameters.feedback);
        // 端点は直交行列。係数の線形補間中もa²+b²<=1となり、時変状態を増幅しない。
        for memory in &mut s.stages {
            for ch in 0..2 {
                let x = wet[ch];
                wet[ch] = a * x + b * memory[ch];
                memory[ch] = b * x - a * memory[ch];
            }
        }
        s.previous = wet;
        let output = std::array::from_fn(|ch| input[ch] + (wet[ch] - input[ch]) * s.parameters.mix);
        s.lfo[0] += s.epsilon * s.lfo[1];
        s.lfo[1] -= s.epsilon * s.lfo[0];
        s.remaining -= 1;
        s.coefficient = if s.remaining == 0 {
            s.target
        } else {
            std::array::from_fn(|i| s.coefficient[i] + s.step[i])
        };
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> PhaserParameters {
        PhaserParameters {
            center_hz: 1000.0,
            depth_cents: 0.0,
            rate_hz: 0.5,
            feedback: 0.0,
            mix: 1.0,
        }
    }
    #[test]
    #[cfg_attr(miri, ignore)] // 長い数値応答の照合は実CPUで行う。
    fn fixed_allpass_preserves_impulse_energy_and_channel_independence() {
        for rate in [44100.0, 48000.0, 96000.0, 192000.0] {
            let mut p = Phaser::new(settings());
            assert!(p.prepare(rate));
            let mut energy = 0.0;
            for n in 0..32768 {
                let y = p.process_sample(if n == 0 { [1.0, 0.0] } else { [0.0; 2] });
                energy += y[0] * y[0];
                assert_eq!(y[1], 0.0);
            }
            assert!((energy - 1.0).abs() < 1e-10);
        }
    }
    #[test]
    #[cfg_attr(miri, ignore)] // 長い数値応答の照合は実CPUで行う。
    fn fixed_response_matches_six_analytic_allpass_stages() {
        for frequency in [100.0, 1000.0, 6000.0] {
            let mut p = Phaser::new(settings());
            assert!(p.prepare(48000.0));
            let w = std::f64::consts::TAU * frequency / 48000.0;
            let a = p.state.coefficient[0];
            let nr = a + w.cos();
            let ni = -w.sin();
            let dr = 1.0 + a * w.cos();
            let di = -a * w.sin();
            let norm = dr * dr + di * di;
            let re = (nr * dr + ni * di) / norm;
            let im = (ni * dr - nr * di) / norm;
            let phase = im.atan2(re) * 6.0;
            for n in 0..10000 {
                let x = (w * n as f64).sin();
                let y = p.process_sample([x, -x]);
                if n > 5000 {
                    assert!((y[0] - (w * n as f64 + phase).sin()).abs() < 1e-10);
                    assert_eq!(y[0], -y[1]);
                }
            }
        }
    }
    #[test]
    #[cfg_attr(miri, ignore)] // 長い数値応答の照合は実CPUで行う。
    fn moving_coefficients_are_contractive_and_silence_decays() {
        let mut settings = settings();
        settings.feedback = 0.95;
        settings.depth_cents = 2400.0;
        settings.rate_hz = 5.0;
        let mut p = Phaser::new(settings);
        for n in 0..100000 {
            if n % 71 == 0 {
                settings.center_hz = 4100.0 - settings.center_hz;
                settings.feedback = -settings.feedback;
                p.set_parameters(settings);
            }
            let y = p.process_sample(if n < 10000 {
                [(n as f64 * 0.3).sin(); 2]
            } else {
                [0.0; 2]
            });
            let c = p.state.coefficient;
            assert!(c[0] * c[0] + c[1] * c[1] <= 1.0 + 1e-14);
            assert!(y.into_iter().all(|v| v.is_finite() && v.abs() < 20.0));
            if n > 99000 {
                assert!(y[0].abs() < 1e-10);
            }
        }
    }
    #[test]
    fn reset_and_rate_change_preserve_contracts() {
        let mut s = settings();
        let mut p = Phaser::new(s);
        for _ in 0..100 {
            p.process_sample([1.0; 2]);
        }
        let sine = p.state.lfo[0];
        let cosine = (p.state.lfo[1] + sine * p.state.half[0]) / p.state.half[1];
        s.rate_hz = 5.0;
        p.set_parameters(s);
        assert_eq!(p.state.lfo[0], sine);
        assert!(
            ((p.state.lfo[1] + sine * p.state.half[0]) / p.state.half[1] - cosine).abs() < 1e-14
        );
        p.reset();
        assert_eq!(p.process_sample([0.0; 2]), [0.0; 2]);
        for rate in [0.0, 44099.0, f64::NAN, f64::INFINITY] {
            assert!(!p.prepare(rate));
        }
    }
}

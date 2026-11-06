// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 線形補間する短い遅延と、遅延出力のフィードバックによるFlanger。

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlangerParameters {
    /// Offは原音を返す。
    pub enabled: bool,
    /// 遅延中心（1〜10 ms）。
    pub time_ms: f64,
    /// 遅延の変調幅（0〜0.95 ms）。
    pub depth_ms: f64,
    /// 左右共通LFOの周波数（0.05〜5 Hz）。
    pub rate_hz: f64,
    /// 遅延出力を入力へ戻す倍率（−0.95〜0.95）。
    pub feedback: f64,
    /// 原音0、処理音1の線形混合率。
    pub mix: f64,
}

#[cfg_attr(test, derive(Clone))]
struct FlangerState {
    /// setupで確保する左右独立の履歴。長さは2の累乗。
    buffer: Vec<[f64; 2]>,
    /// 現在入力の書込み位置。
    write: usize,
    /// reset以降の有効履歴数。
    valid: usize,
    /// ホストのsample rate（Hz）。
    sample_rate: f64,
    /// 適用中の設定。
    parameters: FlangerParameters,
    /// 遅延中心・変調幅（sample）。
    delay: [f64; 2],
    /// f64 MCFの正弦とずれた余弦。
    lfo: [f64; 2],
    /// MCFの回転係数2*sin(pi*rate/Fs)。
    epsilon: f64,
    /// epsilon/2とsqrt(1−(epsilon/2)^2)。
    half: [f64; 2],
}

#[cfg_attr(test, derive(Clone))]
pub struct Flanger {
    state: FlangerState,
}

impl Flanger {
    const MAX_FRAMES: usize = 1 << 20;

    pub fn new(parameters: FlangerParameters) -> Self {
        let mut result = Self {
            state: FlangerState {
                buffer: vec![[0.0; 2]; 512],
                write: 0,
                valid: 0,
                sample_rate: 44100.0,
                parameters,
                delay: [0.0; 2],
                lfo: [0.0, 1.0],
                epsilon: 0.0,
                half: [0.0, 1.0],
            },
        };
        result.update_delay();
        result.update_rate();
        result
    }

    /// setupで最大16 MiBまでの履歴を確保する。音声処理中は呼ばない。
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        let required = (sample_rate * 0.01095).ceil() + 2.0;
        if !sample_rate.is_finite() || sample_rate < 44100.0 || required > Self::MAX_FRAMES as f64 {
            return false;
        }
        let Some(length) = (required as usize).checked_next_power_of_two() else {
            return false;
        };
        if length > Self::MAX_FRAMES {
            return false;
        }
        if length > self.state.buffer.len()
            && self
                .state
                .buffer
                .try_reserve_exact(length - self.state.buffer.len())
                .is_err()
        {
            return false;
        }
        self.state.buffer.resize(length, [0.0; 2]);
        self.state.sample_rate = sample_rate;
        self.update_delay();
        self.update_rate();
        self.reset();
        true
    }

    pub fn enabled(&self) -> bool {
        self.state.parameters.enabled
    }
    pub fn set_parameters(&mut self, p: FlangerParameters) {
        let old = self.state.parameters;
        self.state.parameters = p;
        if p.time_ms != old.time_ms || p.depth_ms != old.depth_ms {
            self.update_delay();
        }
        if p.rate_hz != old.rate_hz {
            self.update_rate();
        }
        if p.enabled != old.enabled {
            self.reset();
        }
    }
    fn update_delay(&mut self) {
        self.state.delay = [
            self.state.parameters.time_ms * self.state.sample_rate * 0.001,
            self.state.parameters.depth_ms * self.state.sample_rate * 0.001,
        ];
    }
    fn update_rate(&mut self) {
        let s = &mut self.state;
        let physical_cosine = (s.lfo[1] + s.lfo[0] * s.half[0]) / s.half[1];
        let h = (std::f64::consts::PI * s.parameters.rate_hz / s.sample_rate).sin();
        let g = (1.0 - h * h).sqrt();
        s.epsilon = 2.0 * h;
        s.half = [h, g];
        s.lfo[1] = physical_cosine * g - s.lfo[0] * h;
    }
    /// 古い領域をゼロ扱いし、音声スレッドでの全領域消去を避ける。
    pub fn reset(&mut self) {
        self.state.write = 0;
        self.state.valid = 0;
        self.state.lfo = [0.0, self.state.half[1]];
    }

    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        if !self.enabled() {
            return input;
        }
        let s = &mut self.state;
        let mask = s.buffer.len() - 1;
        s.buffer[s.write] = input;
        s.valid = (s.valid + 1).min(s.buffer.len());
        let delay = s.delay[0] + s.delay[1] * s.lfo[0];
        let whole = delay as usize;
        let fraction = delay - whole as f64;
        let a = if whole < s.valid {
            s.buffer[s.write.wrapping_sub(whole) & mask]
        } else {
            [0.0; 2]
        };
        let b = if whole + 1 < s.valid {
            s.buffer[s.write.wrapping_sub(whole + 1) & mask]
        } else {
            [0.0; 2]
        };
        let output = std::array::from_fn(|ch| {
            let wet = a[ch] + (b[ch] - a[ch]) * fraction;
            s.buffer[s.write][ch] = input[ch] + s.parameters.feedback * wet;
            input[ch] + (wet - input[ch]) * s.parameters.mix
        });
        s.lfo[0] += s.epsilon * s.lfo[1];
        s.lfo[1] -= s.epsilon * s.lfo[0];
        s.write = (s.write + 1) & mask;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> FlangerParameters {
        FlangerParameters {
            enabled: true,
            time_ms: 1.0,
            depth_ms: 0.0,
            rate_hz: 0.25,
            feedback: 0.5,
            mix: 1.0,
        }
    }
    #[test]
    fn delayed_feedback_has_the_expected_sign_and_time() {
        for feedback in [-0.95, 0.0, 0.5, 0.95] {
            let mut p = settings();
            p.feedback = feedback;
            let mut f = Flanger::new(p);
            assert!(f.prepare(48000.0));
            for n in 0..500 {
                let y = f.process_sample(if n == 0 { [1.0, -0.5] } else { [0.0; 2] });
                let expected = if n > 0 && n % 48 == 0 {
                    feedback.powi(n / 48 - 1)
                } else {
                    0.0
                };
                assert!((y[0] - expected).abs() < 1e-12);
                assert!((y[1] + expected * 0.5).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn bypass_reset_and_rate_changes_preserve_contracts() {
        let mut p = settings();
        let mut f = Flanger::new(p);
        for _ in 0..1000 {
            f.process_sample([1.0; 2]);
        }
        let sine = f.state.lfo[0];
        let cosine = (f.state.lfo[1] + sine * f.state.half[0]) / f.state.half[1];
        p.rate_hz = 5.0;
        f.set_parameters(p);
        assert_eq!(f.state.lfo[0], sine);
        assert!(
            ((f.state.lfo[1] + sine * f.state.half[0]) / f.state.half[1] - cosine).abs() < 1e-14
        );
        p.enabled = false;
        f.set_parameters(p);
        assert_eq!(f.process_sample([0.2, -0.4]), [0.2, -0.4]);
        p.enabled = true;
        f.set_parameters(p);
        for _ in 0..1000 {
            assert_eq!(f.process_sample([0.0; 2]), [0.0; 2]);
        }
    }
    #[test]
    #[cfg_attr(miri, ignore)] // 長い数値応答の照合は実CPUで行う。
    fn moving_delay_and_feedback_remain_bounded_without_allocation() {
        for rate in [44100.0, 48000.0, 96000.0, 192000.0] {
            let mut p = settings();
            p.depth_ms = 0.95;
            p.rate_hz = 5.0;
            p.feedback = 0.95;
            let mut f = Flanger::new(p);
            assert!(f.prepare(rate));
            let capacity = f.state.buffer.capacity();
            for n in 0..30000 {
                if n % 257 == 0 {
                    p.feedback = -p.feedback;
                    p.time_ms = 11.0 - p.time_ms;
                    f.set_parameters(p);
                }
                let y = f.process_sample([(n as f64 * 0.1).sin(); 2]);
                assert!(y.into_iter().all(|v| v.is_finite() && v.abs() <= 20.0));
            }
            assert_eq!(f.state.buffer.capacity(), capacity);
        }
    }
    #[test]
    fn excessive_or_invalid_sample_rates_are_rejected() {
        let mut f = Flanger::new(settings());
        for rate in [0.0, 44099.0, f64::NAN, f64::INFINITY, 1e30] {
            assert!(!f.prepare(rate));
        }
        assert!(f.prepare(192000.0));
    }
}

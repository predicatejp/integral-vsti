// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 左右連動のsample-peak limiter。5 msの先読みと矩形平均を使う。

pub const LOOKAHEAD_MS: f64 = 5.0;

/// 包絡の下降時定数。操作パラメーターへは公開しない。
pub const RELEASE_MS: f64 = 100.0;

#[derive(Clone, Copy, Default)]
struct Frame {
    /// 増幅・補間をしない原音の純遅延。
    audio: [f64; 2],
    /// このsample区間で積分した包絡の平均振幅。
    mean: f64,
}

#[cfg_attr(test, derive(Clone))]
struct LimiterState {
    /// Offでも遅延・包絡履歴を維持する。
    enabled: bool,
    /// 5 msを切り上げた長さの音声・包絡履歴。確保は初期化・setup時だけ。
    history: Vec<Frame>,
    /// 次に交換する最古の履歴の位置。
    position: usize,
    /// 減少順の最大値queue（振幅、入力sample番号）。添字用に2の冪長を確保する。
    peaks: Vec<(f64, u64)>,
    /// 最大ピークを保持するqueueの先頭。
    head: usize,
    /// queue内の有効なピーク数。
    length: usize,
    /// 現在の入力sample番号。周回差で履歴の有効期限を判定する。
    frame: u64,
    /// 直前のsample区間末尾の包絡と、平均窓の積算値。
    envelope: f64,
    sum: f64,
    /// Releaseの1sample残留係数と、区間平均用の積分係数。
    release: f64,
    release_mean: f64,
}

#[cfg_attr(test, derive(Clone))]
pub struct Limiter {
    state: LimiterState,
}

impl Limiter {
    pub fn new() -> Self {
        let size = (44100.0 * LOOKAHEAD_MS / 1000.0).ceil() as usize;
        let mut result = Self {
            state: LimiterState {
                enabled: false,
                history: vec![Frame::default(); size],
                position: 0,
                peaks: vec![(0.0, 0); (size + 1).next_power_of_two()],
                head: 0,
                length: 0,
                frame: 0,
                envelope: 0.0,
                sum: 0.0,
                release: 0.0,
                release_mean: 0.0,
            },
        };
        result.update_release(44100.0);
        result
    }

    /// ホストが音声処理を停止している間だけ呼ぶ。失敗時は元の状態を保持する。
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        let size = (sample_rate * LOOKAHEAD_MS / 1000.0).ceil();
        // 遅延履歴の資源上限。入力レートの検証と、サイズ変換前の上限確認を兼ねる。
        if !sample_rate.is_finite() || sample_rate < 44100.0 || size > 65536.0 {
            return false;
        }
        let size = size as usize;
        if size != self.state.history.len() {
            let mut history = Vec::new();
            let mut peaks = Vec::new();
            if history.try_reserve_exact(size).is_err()
                || peaks
                    .try_reserve_exact((size + 1).next_power_of_two())
                    .is_err()
            {
                return false;
            }
            history.resize(size, Frame::default());
            peaks.resize((size + 1).next_power_of_two(), (0.0, 0));
            self.state.history = history;
            self.state.peaks = peaks;
        }
        self.update_release(sample_rate);
        self.reset();
        true
    }

    pub fn latency_samples(&self) -> usize {
        self.state.history.len()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.state.enabled = enabled;
    }

    fn update_release(&mut self, sample_rate: f64) {
        let step = 1000.0 / (RELEASE_MS * sample_rate);
        // 端点を平均する代わりに指数減衰を区間積分し、レート依存の誤差を除く。
        self.state.release = (-step).exp();
        self.state.release_mean = -(-step).exp_m1() / step;
    }

    pub fn reset(&mut self) {
        self.state.history.fill(Frame::default());
        self.state.position = 0;
        self.state.head = 0;
        self.state.length = 0;
        self.state.frame = 0;
        self.state.envelope = 0.0;
        self.state.sum = 0.0;
    }

    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        let s = &mut self.state;
        let peak = input[0].abs().max(input[1].abs());
        let size = s.history.len();
        let mask = s.peaks.len() - 1;
        if s.length > 0 && s.frame.wrapping_sub(s.peaks[s.head].1) > size as u64 {
            s.head = (s.head + 1) & mask;
            s.length -= 1;
        }
        while s.length > 0 {
            let tail = (s.head + s.length - 1) & mask;
            if s.peaks[tail].0 > peak {
                break;
            }
            s.length -= 1;
        }
        s.peaks[(s.head + s.length) & mask] = (peak, s.frame);
        s.length += 1;
        s.frame = s.frame.wrapping_add(1);
        let maximum = s.peaks[s.head].0;
        let excess = (s.envelope - maximum).max(0.0);
        s.envelope = maximum + s.release * excess;
        let mean = maximum + s.release_mean * excess;

        // n−D..n−1のD区間を平均する。全区間の最大値窓が出力sample n−Dを
        // 含むため、sum/Dはその入力ピーク以上となり、Ceilingを守れる。
        let gain = if s.sum > size as f64 {
            size as f64 / s.sum
        } else {
            1.0
        };
        let delayed = std::mem::replace(&mut s.history[s.position], Frame { audio: input, mean });
        s.sum += mean - delayed.mean;
        s.position = if s.position + 1 == size {
            0
        } else {
            s.position + 1
        };
        if s.enabled {
            delayed.audio.map(|x| x * gain)
        } else {
            delayed.audio
        }
    }

    pub fn process_samples(&mut self, samples: &mut [[f64; 2]]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_ms_delay_is_exact_and_bypass_preserves_samples() {
        for (rate, delay) in [
            (44100., 221),
            (48000., 240),
            (88200., 441),
            (96000., 480),
            (192000., 960),
        ] {
            let mut limiter = Limiter::new();
            assert!(limiter.prepare(rate));
            assert_eq!(limiter.latency_samples(), delay);
            assert!(delay as f64 / rate >= LOOKAHEAD_MS / 1000.0);
            assert!(delay as f64 / rate - LOOKAHEAD_MS / 1000.0 < 1.0 / rate);
            let mut expected = std::collections::VecDeque::from(vec![[0.; 2]; delay]);
            for n in 0..delay * 3 {
                let x = [(n as f64 * 0.13).sin() * 3.0, (n as f64 * 0.07).cos()];
                expected.push_back(x);
                assert_eq!(limiter.process_sample(x), expected.pop_front().unwrap());
            }
            limiter.reset();
            assert_eq!(limiter.process_sample([0.; 2]), [0.; 2]);
            assert!(!limiter.prepare(f64::INFINITY));
            assert!(!limiter.prepare(44099.));
            assert_eq!(limiter.latency_samples(), delay);
        }
    }

    #[test]
    fn peak_bound_and_stereo_link_survive_switches_and_sample_counter_wrap() {
        for rate in [44100., 48000., 96000., 192000.] {
            let mut limiter = Limiter::new();
            assert!(limiter.prepare(rate));
            limiter.state.frame = u64::MAX - 500;
            for n in 0..12000 {
                limiter.set_enabled(n % 997 > 199);
                let x = if n % 501 == 0 {
                    50.0
                } else {
                    (n as f64 * 0.13).sin() * (n % 127) as f64
                };
                let y = limiter.process_sample([x, -x * 0.25]);
                assert!(y.into_iter().all(f64::is_finite));
                assert_eq!(y[1], -y[0] * 0.25);
                if limiter.state.enabled {
                    assert!(
                        y[0].abs() <= 1.0 + 1e-11,
                        "rate={rate}, n={n}, output={y:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rectangular_pulse_matches_continuous_time_gain_at_each_rate() {
        for rate in [44100., 48000., 88200., 96000., 192000.] {
            let mut limiter = Limiter::new();
            assert!(limiter.prepare(rate));
            limiter.set_enabled(true);
            let delay = limiter.latency_samples();
            let duration = (rate * 0.010) as usize;
            let window = delay as f64 / rate;
            let hold = (duration + delay) as f64 / rate;
            let tau = RELEASE_MS / 1000.0;
            let integral = |t: f64| {
                2.0 * (t.clamp(0.0, hold) + tau * (-(-(t - hold).max(0.0) / tau).exp_m1()))
            };
            for n in 0..(rate * 0.3) as usize {
                let t = n as f64 / rate;
                let mean = (integral(t) - integral(t - window)) / window;
                let gain = if mean > 1.0 { mean.recip() } else { 1.0 };
                let actual_mean = limiter.state.sum / delay as f64;
                assert!((actual_mean - mean).abs() < 2e-11, "rate={rate}, n={n}");
                let x = if n < duration { 2.0 } else { 0.0 };
                let y = limiter.process_sample([x, x * 0.5]);
                let expected = if (delay..delay + duration).contains(&n) {
                    2.0 * gain
                } else {
                    0.0
                };
                assert!((y[0] - expected).abs() < 2e-11);
            }
        }
    }

    #[test]
    fn variable_blocks_match_samples_and_reset_clears_all_history() {
        let mut scalar = Limiter::new();
        assert!(scalar.prepare(48000.));
        scalar.set_enabled(true);
        let mut blocks = scalar.clone();
        for step in 0..32 {
            scalar.set_enabled(step % 3 != 0);
            blocks.set_enabled(step % 3 != 0);
            let mut samples: Vec<_> = (0..[0, 1, 31, 257, 3, 960, 63, 32][step % 8])
                .map(|n| [(n as f64 * 0.07).sin() * 3.0, (n as f64 * 0.17).cos() * 2.0])
                .collect();
            let expected: Vec<_> = samples.iter().map(|&x| scalar.process_sample(x)).collect();
            blocks.process_samples(&mut samples);
            assert_eq!(samples, expected);
        }
        blocks.reset();
        for _ in 0..1000 {
            assert_eq!(blocks.process_sample([0.; 2]), [0.; 2]);
        }
        assert!(blocks.prepare(96000.));
        assert_eq!(blocks.latency_samples(), 480);
        assert_eq!(blocks.process_sample([0.; 2]), [0.; 2]);
    }
}

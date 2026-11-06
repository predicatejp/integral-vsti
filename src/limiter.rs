// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 左右sample peakの最大値窓と矩形平均による固定先読みリミッター。

/// 48 kHzで5 ms。Offも同じサンプル数だけ遅らせる。
pub const LATENCY: usize = 240;
const WINDOW: usize = LATENCY + 1;

#[derive(Clone, Copy, PartialEq)]
pub struct LimiterParameters {
    /// Offでは同じ遅延を通した原音を返す。
    pub enabled: bool,
    /// Master前のsample peak上限（dBFS）。
    pub ceiling_db: f64,
    /// 検出包絡の下降時定数（ms）。
    pub release_ms: f64,
    /// 遅延へ入る時点で処理音へ掛ける増幅量（dB）。
    pub input_gain_db: f64,
}

#[cfg_attr(test, derive(Clone))]
struct LimiterState {
    /// 現在の操作値。切替時も遅延と検出履歴を継続する。
    parameters: LimiterParameters,
    /// release係数を計算したサンプルレート。
    sample_rate: f64,
    /// 原音とInput Gain適用後の音声を各240 sample保持する。
    dry: [[f64; 2]; LATENCY],
    wet: [[f64; 2]; LATENCY],
    /// 音声遅延の次の読出し・書込み位置。
    delay_position: usize,
    /// 減少順の最大値queue。各値は一度だけ追加・削除される。
    peaks: [f64; WINDOW],
    /// 各peakの入力sample番号。
    indices: [u64; WINDOW],
    /// queue先頭と有効要素数。
    head: usize,
    length: usize,
    /// 現在の入力sample番号。差はwrapping_subで比較する。
    frame: u64,
    /// 即時上昇・一次releaseの包絡。
    envelope: f64,
    /// 矩形平均の履歴・総和・次の交換位置。
    mean: [f64; WINDOW],
    sum: f64,
    mean_position: usize,
    /// 操作値変更時だけ計算する線形上限・増幅率・release残留係数。
    ceiling: f64,
    input_gain: f64,
    release: f64,
}

#[cfg_attr(test, derive(Clone))]
pub struct Limiter {
    /// 固定サイズの音声・検出・平均履歴。
    state: LimiterState,
}

impl Limiter {
    pub fn new(parameters: LimiterParameters, sample_rate: f64) -> Self {
        Self {
            state: LimiterState {
                parameters,
                sample_rate,
                dry: [[0.0; 2]; LATENCY],
                wet: [[0.0; 2]; LATENCY],
                delay_position: 0,
                peaks: [0.0; WINDOW],
                indices: [0; WINDOW],
                head: 0,
                length: 0,
                frame: 0,
                envelope: 0.0,
                mean: [0.0; WINDOW],
                sum: 0.0,
                mean_position: 0,
                ceiling: 10.0_f64.powf(parameters.ceiling_db / 20.0),
                input_gain: 10.0_f64.powf(parameters.input_gain_db / 20.0),
                release: coefficient(parameters.release_ms, sample_rate),
            },
        }
    }

    pub fn set_parameters(&mut self, parameters: LimiterParameters, sample_rate: f64) {
        let old = self.state.parameters;
        if parameters.ceiling_db != old.ceiling_db {
            self.state.ceiling = 10.0_f64.powf(parameters.ceiling_db / 20.0);
        }
        if parameters.input_gain_db != old.input_gain_db {
            self.state.input_gain = 10.0_f64.powf(parameters.input_gain_db / 20.0);
        }
        if parameters.release_ms != old.release_ms || sample_rate != self.state.sample_rate {
            self.state.release = coefficient(parameters.release_ms, sample_rate);
        }
        self.state.parameters = parameters;
        self.state.sample_rate = sample_rate;
    }

    pub fn reset(&mut self) {
        self.state.dry.fill([0.0; 2]);
        self.state.wet.fill([0.0; 2]);
        self.state.delay_position = 0;
        self.state.head = 0;
        self.state.length = 0;
        self.state.frame = 0;
        self.state.envelope = 0.0;
        self.state.mean.fill(0.0);
        self.state.sum = 0.0;
        self.state.mean_position = 0;
    }

    /// inputはMaster前、bypassは元のMaster時刻を反映した信号。
    /// On時の戻り値にだけ呼出し元が現在のMasterを適用する。
    pub fn process_sample(&mut self, input: [f64; 2], bypass: [f64; 2]) -> [f64; 2] {
        let input = input.map(|x| x * self.state.input_gain);
        let dry = std::mem::replace(&mut self.state.dry[self.state.delay_position], bypass);
        let wet = std::mem::replace(&mut self.state.wet[self.state.delay_position], input);
        self.state.delay_position = (self.state.delay_position + 1) % LATENCY;
        let peak = input[0].abs().max(input[1].abs());
        if self.state.length > 0
            && self
                .state
                .frame
                .wrapping_sub(self.state.indices[self.state.head])
                > LATENCY as u64
        {
            self.state.head = (self.state.head + 1) % WINDOW;
            self.state.length -= 1;
        }
        while self.state.length > 0 {
            let tail = (self.state.head + self.state.length - 1) % WINDOW;
            if self.state.peaks[tail] > peak {
                break;
            }
            self.state.length -= 1;
        }
        let tail = (self.state.head + self.state.length) % WINDOW;
        self.state.peaks[tail] = peak;
        self.state.indices[tail] = self.state.frame;
        self.state.length += 1;
        self.state.frame = self.state.frame.wrapping_add(1);
        let maximum = self.state.peaks[self.state.head];
        self.state.envelope =
            maximum.max(maximum + self.state.release * (self.state.envelope - maximum));
        let old = std::mem::replace(
            &mut self.state.mean[self.state.mean_position],
            self.state.envelope,
        );
        self.state.sum += self.state.envelope - old;
        self.state.mean_position = (self.state.mean_position + 1) % WINDOW;
        // 各窓が出力時刻のpeakを含むため、平均後もその振幅以上になる。
        let envelope = self.state.sum / WINDOW as f64;
        let gain = if envelope > self.state.ceiling {
            self.state.ceiling / envelope
        } else {
            1.0
        };
        if self.state.parameters.enabled {
            wet.map(|x| x * gain)
        } else {
            dry
        }
    }
}

fn coefficient(ms: f64, rate: f64) -> f64 {
    if ms == 0.0 {
        0.0
    } else {
        (-1000.0 / (ms * rate)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parameters() -> LimiterParameters {
        LimiterParameters {
            enabled: true,
            ceiling_db: -1.0,
            release_ms: 100.0,
            input_gain_db: 6.0,
        }
    }

    #[test]
    fn bounds_samples_through_startup_and_rapid_level_changes() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let mut p = parameters();
            let mut limiter = Limiter::new(p, rate);
            let mut history = std::collections::VecDeque::from(vec![[0.0; 2]; LATENCY]);
            for n in 0..100_000 {
                p.ceiling_db = if n % 997 < 500 { -1.0 } else { -12.0 };
                p.input_gain_db = if n % 601 < 300 { 0.0 } else { 24.0 };
                p.release_ms = if n % 577 < 200 { 0.0 } else { 100.0 };
                limiter.set_parameters(p, rate);
                let x = if n % 479 < 40 {
                    0.0
                } else {
                    ((n as f64 * 0.7).sin() * 3.0).max(-2.0)
                };
                let input = [x, -0.25 * x];
                let output = limiter.process_sample(input, input);
                let ceiling = 10.0_f64.powf(p.ceiling_db / 20.0);
                assert!(
                    output[0].abs() <= ceiling + 1e-10,
                    "{n}: {} > {ceiling}",
                    output[0]
                );
                assert_eq!(output[1], -0.25 * output[0]);
                assert!(output.into_iter().all(f64::is_finite));
                history.push_back(input);
                let delayed = history.pop_front().unwrap();
                if n < LATENCY {
                    assert_eq!(output, delayed);
                }
            }
            limiter.reset();
            for _ in 0..LATENCY * 2 {
                assert_eq!(limiter.process_sample([0.0; 2], [0.0; 2]), [0.0; 2]);
            }
        }
    }

    #[test]
    fn bypass_is_exact_pure_delay_and_switch_keeps_the_signal_time() {
        let mut p = parameters();
        p.enabled = false;
        let mut limiter = Limiter::new(p, 48000.0);
        let mut reference = std::collections::VecDeque::from(vec![[0.0; 2]; LATENCY]);
        for n in 0..2000 {
            let input = [(n as f64 * 0.03).sin() * 0.01, 0.0];
            reference.push_back(input);
            let expected = reference.pop_front().unwrap();
            assert_eq!(limiter.process_sample(input, input), expected);
        }
        p.enabled = true;
        p.input_gain_db = 0.0;
        // ゼロで埋め直さず、それまで継続した処理音を遅延位置から返す。
        limiter.set_parameters(p, 48000.0);
        assert_ne!(limiter.process_sample([0.0; 2], [0.0; 2]), [0.0; 2]);
    }
}

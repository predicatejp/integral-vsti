// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 共通ステレオDelay。時間変更は50 msの2読出し位置の線形クロスフェード。

pub const DIVISIONS: [f64; 9] = [0.25, 1.0 / 3.0, 0.5, 0.75, 2.0 / 3.0, 1.0, 1.5, 2.0, 4.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayParameters {
    /// 0: bypass、1: ms指定、2: ホストの現在BPMと音価。
    pub mode: u32,
    /// 非同期モードの遅延時間。1〜8000 ms。
    pub time_ms: f64,
    /// 四分音符を1とするDIVISIONSの選択番号。
    pub division: usize,
    /// 再帰入力倍率。0〜0.95。
    pub feedback: f64,
    /// Wet混合率。0〜1。
    pub mix: f64,
    /// trueなら左右を交換してフィードバックする。
    pub ping_pong: bool,
}

impl Default for DelayParameters {
    fn default() -> Self {
        Self {
            mode: 0,
            time_ms: 250.0,
            division: 2,
            feedback: 0.35,
            mix: 0.25,
            ping_pong: false,
        }
    }
}

#[derive(Clone)]
pub struct DelayState {
    /// 8秒分以上を確保した2の冪長リング。setup時だけ再確保する。
    buffer: Vec<[f64; 2]>,
    /// 書込位置と、有効な過去フレーム数。resetは有効数だけを0にする。
    write: usize,
    valid: usize,
    /// 入力レート。時間とサンプル数の換算に使用。
    sample_rate: f64,
    /// 最後に受信した有効BPM。取得前の既定は120。
    tempo: f64,
    parameters: DelayParameters,
    /// クロスフェード元・先と、後続変更の最新目標（サンプル単位）。
    from: f64,
    to: f64,
    requested: f64,
    /// 0なら定常。1..=fade_lengthならクロスフェード中。
    fade: usize,
    fade_length: usize,
}

#[derive(Clone)]
pub struct Delay {
    state: DelayState,
}

impl Delay {
    pub fn new() -> Self {
        let mut result = Self {
            state: DelayState {
                buffer: Vec::new(),
                write: 0,
                valid: 0,
                sample_rate: 44100.0,
                tempo: 120.0,
                parameters: DelayParameters::default(),
                from: 11025.0,
                to: 11025.0,
                requested: 11025.0,
                fade: 0,
                fade_length: 2205,
            },
        };
        assert!(result.prepare(44100.0));
        result
    }

    /// 音声処理開始前だけ呼ぶ。共有リングの資源上限は64 MiB。
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        let required = (sample_rate * 8.0).ceil() + 2.0;
        if !sample_rate.is_finite() || sample_rate < 44100.0 || required > (1 << 22) as f64 {
            return false;
        }
        let size = (required as usize).next_power_of_two();
        if size != self.state.buffer.len() {
            let mut buffer = Vec::new();
            if buffer.try_reserve_exact(size).is_err() {
                return false;
            }
            buffer.resize(size, [0.0; 2]);
            self.state.buffer = buffer;
        }
        self.state.sample_rate = sample_rate;
        self.state.fade_length = (sample_rate * 0.05).round() as usize;
        self.reset();
        true
    }

    pub fn enabled(&self) -> bool {
        self.state.parameters.mode != 0
    }

    pub fn set_parameters(&mut self, parameters: DelayParameters) {
        if self.state.parameters == parameters {
            return;
        }
        let reset = self.state.parameters.mode == 0 || parameters.mode == 0;
        self.state.parameters = parameters;
        if reset {
            self.reset();
        } else {
            self.update_time();
        }
    }

    /// 有効フラグを確認したホストBPMを受け取る。欠測・不正値は保持する。
    pub fn set_tempo(&mut self, tempo: f64) {
        if tempo.is_finite() && tempo > 0.0 && tempo != self.state.tempo {
            self.state.tempo = tempo;
            if self.state.parameters.mode == 2 {
                self.update_time();
            }
        }
    }

    pub fn reset(&mut self) {
        self.state.write = 0;
        self.state.valid = 0;
        self.state.fade = 0;
        self.update_time();
    }

    fn update_time(&mut self) {
        let p = self.state.parameters;
        let seconds = if p.mode == 2 {
            60.0 * DIVISIONS[p.division] / self.state.tempo
        } else {
            p.time_ms * 0.001
        };
        self.state.requested =
            (seconds * self.state.sample_rate).clamp(1.0, 8.0 * self.state.sample_rate);
        if self.state.valid == 0 {
            self.state.from = self.state.requested;
            self.state.to = self.state.requested;
            self.state.fade = 0;
        } else if self.state.fade == 0 && self.state.requested != self.state.to {
            self.state.from = self.state.to;
            self.state.to = self.state.requested;
            self.state.fade = 1;
        }
    }

    fn read_sample(&self, delay: f64) -> [f64; 2] {
        let whole = delay as usize;
        let frac = delay - whole as f64;
        let mask = self.state.buffer.len() - 1;
        let index = self.state.write.wrapping_sub(whole) & mask;
        let a = if whole <= self.state.valid {
            self.state.buffer[index]
        } else {
            [0.0; 2]
        };
        let b = if whole < self.state.valid {
            self.state.buffer[index.wrapping_sub(1) & mask]
        } else {
            [0.0; 2]
        };
        std::array::from_fn(|ch| a[ch] + frac * (b[ch] - a[ch]))
    }

    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        if !self.enabled() {
            return input;
        }
        let mut wet = self.read_sample(self.state.to);
        if self.state.fade != 0 {
            let old = self.read_sample(self.state.from);
            let weight = (self.state.fade - 1) as f64 / (self.state.fade_length - 1) as f64;
            wet = std::array::from_fn(|ch| old[ch] + weight * (wet[ch] - old[ch]));
            self.state.fade += 1;
            if self.state.fade > self.state.fade_length {
                self.state.fade = 0;
                self.update_time();
            }
        }
        let p = self.state.parameters;
        let swap = usize::from(p.ping_pong);
        self.state.buffer[self.state.write] =
            std::array::from_fn(|ch| input[ch] + p.feedback * wet[ch ^ swap]);
        self.state.write = (self.state.write + 1) & (self.state.buffer.len() - 1);
        self.state.valid = (self.state.valid + 1).min(self.state.buffer.len() - 1);
        std::array::from_fn(|ch| input[ch] + p.mix * (wet[ch] - input[ch]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_delay_and_time_limits() {
        let mut delay = Delay::new();
        delay.set_parameters(DelayParameters {
            mode: 1,
            time_ms: 1.0,
            feedback: 0.0,
            mix: 1.0,
            ..Default::default()
        });
        for n in 0..47 {
            let y = delay.process_sample(if n == 0 { [1.0; 2] } else { [0.0; 2] });
            let expected = match n {
                44 => 0.9,
                45 => 0.1,
                _ => 0.0,
            };
            assert!((y[0] - expected).abs() < 1e-12);
        }
        delay.set_parameters(DelayParameters {
            mode: 2,
            ..Default::default()
        });
        delay.reset();
        delay.set_tempo(f64::MIN_POSITIVE);
        assert_eq!(delay.state.to, 44100.0 * 8.0);
        delay.set_tempo(f64::MAX);
        assert_eq!(delay.state.to, 1.0);
    }

    #[test]
    fn free_and_sync_are_identical_and_feedback_arrives_at_the_expected_time() {
        let mut a = Delay::new();
        let mut b = Delay::new();
        assert!(a.prepare(48000.0) && b.prepare(48000.0));
        let p = DelayParameters {
            mode: 1,
            mix: 1.0,
            feedback: 0.5,
            ..Default::default()
        };
        a.set_parameters(p);
        b.set_parameters(DelayParameters { mode: 2, ..p });
        for n in 0..36001 {
            let input = if n == 0 { [1.0, -1.0] } else { [0.0; 2] };
            let x = a.process_sample(input);
            assert_eq!(x, b.process_sample(input));
            let expected = match n {
                12000 => 1.0,
                24000 => 0.5,
                36000 => 0.25,
                _ => 0.0,
            };
            assert_eq!(x, [expected, -expected]);
        }
    }
    #[test]
    fn missing_tempo_ping_pong_reset_and_rapid_updates() {
        let mut delay = Delay::new();
        let p = DelayParameters {
            mode: 2,
            division: 0,
            ping_pong: true,
            mix: 1.0,
            ..Default::default()
        };
        delay.set_parameters(p);
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            delay.set_tempo(invalid);
        }
        assert_eq!(delay.state.tempo, 120.0);
        delay.set_tempo(60.0);
        assert_eq!(delay.state.to, 11025.0);
        for n in 0..22051 {
            let x = delay.process_sample(if n == 0 { [1.0, 0.0] } else { [0.0; 2] });
            if n == 11025 {
                assert_eq!(x, [1.0, 0.0]);
            }
            if n == 22050 {
                assert_eq!(x, [0.0, 0.35]);
            }
        }
        for n in 0..20000 {
            if n % 7 == 0 {
                delay.set_tempo(30.0 + (n % 241) as f64);
            }
            assert!(
                delay
                    .process_sample([0.25, -0.25])
                    .iter()
                    .all(|x| x.is_finite() && x.abs() < 2.0)
            );
        }
        delay.reset();
        assert_eq!(delay.process_sample([0.0; 2]), [0.0; 2]);
        assert!(!delay.prepare(f64::MAX));
    }
}

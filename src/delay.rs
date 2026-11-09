// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! テンポ同期のステレオDelay。音価・テンポ変更は50 msのクロスフェード。

pub const DIVISIONS: [f64; 10] = [
    0.125,
    0.25,
    1.0 / 3.0,
    0.5,
    2.0 / 3.0,
    0.75,
    1.0,
    1.5,
    2.0,
    4.0,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayParameters {
    /// 四分音符を1とするDIVISIONSの選択番号。
    pub division: usize,
    /// 再帰入力倍率。0〜1。
    pub feedback: f64,
    /// Wet混合率。0〜1。
    pub mix: f64,
}

#[derive(Clone)]
pub struct DelayState {
    /// 8秒分以上を確保した2の冪長リング。setup時だけ再確保する。
    buffer: Vec<[f64; 2]>,
    /// 次の書込位置。
    write: usize,
    /// 有効な過去フレーム数。resetはバッファを走査せず、この値を0にする。
    valid: usize,
    /// 入力レート。時間とサンプル数の換算に使用。
    sample_rate: f64,
    /// 最後に受信した有効BPM。取得前の既定は120。
    tempo: f64,
    /// 音価・帰還倍率・Wet混合率。
    parameters: DelayParameters,
    /// クロスフェード元・先と、後続変更の最新目標（サンプル単位）。
    from: f64,
    to: f64,
    requested: f64,
    /// 0なら定常。1..=fade_lengthならクロスフェード中。
    fade: usize,
    /// 50 msに対応するサンプル数。
    fade_length: usize,
}

#[derive(Clone)]
pub struct Delay {
    /// このコンポーネントの履歴と設定。
    state: DelayState,
}

impl Delay {
    pub fn new(parameters: DelayParameters) -> Self {
        Self {
            state: DelayState {
                buffer: Vec::new(),
                write: 0,
                valid: 0,
                sample_rate: 44100.0,
                tempo: 120.0,
                parameters,
                from: 11025.0,
                to: 11025.0,
                requested: 11025.0,
                fade: 0,
                fade_length: 2205,
            },
        }
    }

    pub fn prepared_for(&self, sample_rate: f64) -> bool {
        !self.state.buffer.is_empty() && self.state.sample_rate == sample_rate
    }

    /// 音声処理開始前だけ呼ぶ。1スロットのリングの資源上限は64 MiB。
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

    pub fn set_parameters(&mut self, parameters: DelayParameters) {
        if self.state.parameters == parameters {
            return;
        }
        let time_changed = self.state.parameters.division != parameters.division;
        self.state.parameters = parameters;
        if time_changed {
            self.update_time();
        }
    }

    /// 有効フラグを確認したホストBPMを受け取る。欠測・不正値は保持する。
    pub fn set_tempo(&mut self, tempo: f64) {
        if tempo.is_finite() && tempo > 0.0 && tempo != self.state.tempo {
            self.state.tempo = tempo;
            self.update_time();
        }
    }

    pub fn reset(&mut self) {
        self.state.write = 0;
        self.state.valid = 0;
        self.state.fade = 0;
        self.update_time();
    }

    fn update_time(&mut self) {
        let seconds = 60.0 * DIVISIONS[self.state.parameters.division] / self.state.tempo;
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
        debug_assert!(!self.state.buffer.is_empty(), "prepare before processing");
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
        self.state.buffer[self.state.write] =
            std::array::from_fn(|ch| input[ch] + p.feedback * wet[ch]);
        self.state.write = (self.state.write + 1) & (self.state.buffer.len() - 1);
        self.state.valid = (self.state.valid + 1).min(self.state.buffer.len() - 1);
        std::array::from_fn(|ch| input[ch] + p.mix * (wet[ch] - input[ch]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn make(rate: f64, division: usize, feedback: f64, mix: f64, tempo: f64) -> Delay {
        let mut delay = Delay::new(DelayParameters {
            division,
            feedback,
            mix,
        });
        assert!(delay.prepare(rate));
        delay.set_tempo(tempo);
        delay
    }

    #[test]
    fn divisions_tempo_and_fractional_stereo_impulses() {
        for rate in [44100.0, 48000.0, 96000.0] {
            for (division, beats) in DIVISIONS.into_iter().enumerate() {
                let mut delay = make(rate, division, 0.0, 1.0, 137.0);
                let samples = 60.0 * beats * rate / 137.0;
                let whole = samples.floor() as usize;
                let fraction = samples.fract();
                for n in 0..=whole + 2 {
                    let input = if n == 0 { [1.0, -0.25] } else { [0.0; 2] };
                    let weight = if n == whole {
                        1.0 - fraction
                    } else if n == whole + 1 {
                        fraction
                    } else {
                        0.0
                    };
                    let actual = delay.process_sample(input);
                    assert!((actual[0] - weight).abs() < 1e-10);
                    assert!((actual[1] + 0.25 * weight).abs() < 1e-10);
                }
            }
        }
    }

    #[test]
    fn full_feedback_repeats_without_decay_and_mix_leaves_dry_untouched() {
        let mut delay = make(48000.0, 0, 1.0, 1.0, 120.0); // 3000 samples
        for n in 0..=530000 {
            let input = if n == 0 { [0.75, -0.25] } else { [0.0; 2] };
            let expected = if n > 0 && n % 3000 == 0 {
                [0.75, -0.25]
            } else {
                [0.0; 2]
            };
            assert_eq!(delay.process_sample(input), expected);
        }
        delay.set_parameters(DelayParameters {
            mix: 0.0,
            ..delay.state.parameters
        });
        for n in 0..6000 {
            let input = [(n as f64 * 0.1).sin(), (n as f64 * 0.13).cos()];
            assert_eq!(delay.process_sample(input), input);
        }
        delay.reset();
        delay.set_parameters(DelayParameters {
            mix: 1.0,
            ..delay.state.parameters
        });
        for _ in 0..6000 {
            assert_eq!(delay.process_sample([0.0; 2]), [0.0; 2]);
        }
    }

    #[test]
    fn invalid_tempo_holds_and_valid_tempo_clamps_to_buffer_bounds() {
        let mut delay = make(48000.0, 0, 0.0, 1.0, 120.0);
        delay.set_tempo(150.0);
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            delay.set_tempo(invalid);
            assert_eq!(delay.state.to, 2400.0);
        }
        delay.set_tempo(f64::MIN_POSITIVE);
        assert_eq!(delay.state.to, 384000.0);
        delay.set_tempo(f64::MAX);
        assert_eq!(delay.state.to, 1.0);
        assert!(!delay.prepare(1e10));
        assert_eq!(delay.state.sample_rate, 48000.0);
        assert!(!delay.prepare(44099.0));
    }

    #[test]
    fn tempo_changes_crossfade_for_fifty_ms_and_finish_at_latest_request() {
        let mut delay = make(48000.0, 0, 0.0, 1.0, 120.0);
        for n in 0..8000 {
            delay.process_sample([n as f64; 2]);
        }
        delay.set_tempo(150.0); // old=3000, new=2400
        for n in 0..2400 {
            if n == 100 {
                delay.set_tempo(100.0);
            } // queue latest=3600
            let old = (8000 + n - 3000) as f64;
            let new = (8000 + n - 2400) as f64;
            let expected = old + (new - old) * n as f64 / 2399.0;
            assert!((delay.process_sample([(8000 + n) as f64; 2])[0] - expected).abs() < 1e-10);
        }
        for n in 0..2400 {
            delay.process_sample([(10400 + n) as f64; 2]);
        }
        assert_eq!(delay.state.fade, 0);
        assert_eq!(delay.state.to, 3600.0);
        assert_eq!(delay.process_sample([12800.0; 2]), [9200.0; 2]);
    }
}

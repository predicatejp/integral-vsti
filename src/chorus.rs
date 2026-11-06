// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 左右1tapの線形補間Chorus。Rateの異なるf64 MCF LFOで遅延を動かす。

/// コントローラーで検証した定常設定。時間はms、RateはHz、Mixは0〜1。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChorusParameters {
    /// Offでは原音をそのまま返す。
    pub enabled: bool,
    /// 遅延の中心（5〜40 ms）。
    pub time_ms: f64,
    /// 中心からの変調幅（0〜5 ms）。
    pub depth_ms: f64,
    /// 左LFOの周波数（0.05〜5 Hz）。右はこの0.83倍。
    pub rate_hz: f64,
    /// 原音0、処理音1の線形混合率。
    pub mix: f64,
}

#[cfg_attr(test, derive(Clone))]
struct ChorusState {
    /// setupで確保した左右独立の音声履歴。長さは2の累乗。
    buffer: Vec<[f64; 2]>,
    /// 次に現在入力を書き込む位置。
    write: usize,
    /// reset以降の有効な履歴数。古い履歴をゼロとして読む。
    valid: usize,
    /// 現在のホストレート（Hz）。
    sample_rate: f64,
    /// 適用済みの設定。
    parameters: ChorusParameters,
    /// 中心と変調幅をsample単位に換算した値。
    delay: [f64; 2],
    /// 各MCFの正弦状態。
    sine: [f64; 2],
    /// MCFのずれた余弦状態。物理余弦は(c + sine*h)/g。
    cosine: [f64; 2],
    /// 1sampleの回転を指定する2*sin(pi*rate/Fs)。
    epsilon: [f64; 2],
    /// epsilon/2とsqrt(1−(epsilon/2)^2)。Rate変更時の座標変換に使う。
    half: [[f64; 2]; 2],
}

/// 音声履歴とLFO状態を所有し、左右1tapを処理する。
#[cfg_attr(test, derive(Clone))]
pub struct Chorus {
    state: ChorusState,
}

impl Chorus {
    /// 最大16 MiBのstereo f64履歴。setupでの過大な資源要求を拒否する。
    const MAX_FRAMES: usize = 1 << 20;

    pub fn new(parameters: ChorusParameters) -> Self {
        let mut result = Self {
            state: ChorusState {
                buffer: vec![[0.0; 2]; 2048],
                write: 0,
                valid: 0,
                sample_rate: 44100.0,
                parameters,
                delay: [0.0; 2],
                sine: [0.0; 2],
                cosine: [1.0, -1.0],
                epsilon: [0.0; 2],
                half: [[0.0, 1.0]; 2],
            },
        };
        result.update_delay();
        result.update_rate();
        result
    }

    /// 音声処理外で呼ぶ。変更したレートに必要な履歴を確保し、音声とLFOを初期化する。
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        let required = (sample_rate * 0.045).ceil() + 2.0;
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

    pub fn set_parameters(&mut self, parameters: ChorusParameters) {
        let restart = parameters.enabled != self.state.parameters.enabled;
        if parameters == self.state.parameters {
            return;
        }
        let previous = self.state.parameters;
        self.state.parameters = parameters;
        if parameters.time_ms != previous.time_ms || parameters.depth_ms != previous.depth_ms {
            self.update_delay();
        }
        if parameters.rate_hz != previous.rate_hz {
            self.update_rate();
        }
        if restart {
            self.reset();
        }
    }

    pub fn enabled(&self) -> bool {
        self.state.parameters.enabled
    }

    fn update_delay(&mut self) {
        let p = self.state.parameters;
        self.state.delay = [
            p.time_ms * self.state.sample_rate * 0.001,
            p.depth_ms * self.state.sample_rate * 0.001,
        ];
    }

    fn update_rate(&mut self) {
        let p = self.state.parameters;
        for (i, ratio) in [1.0, 0.83].into_iter().enumerate() {
            let old = self.state.half[i];
            let physical_cosine = (self.state.cosine[i] + self.state.sine[i] * old[0]) / old[1];
            let h = (std::f64::consts::PI * p.rate_hz * ratio / self.state.sample_rate).sin();
            let g = (1.0 - h * h).sqrt();
            self.state.epsilon[i] = 2.0 * h;
            self.state.half[i] = [h, g];
            // Rate変更で保持した位相を変えず、新しいMCFの座標へ移す。
            self.state.cosine[i] = physical_cosine * g - self.state.sine[i] * h;
        }
    }

    /// 履歴領域を走査せず無効化する。次の有効化で古い残響を再生しない。
    pub fn reset(&mut self) {
        self.state.write = 0;
        self.state.valid = 0;
        self.state.sine = [0.0; 2];
        self.state.cosine = [self.state.half[0][1], -self.state.half[1][1]];
    }

    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        if !self.state.parameters.enabled {
            return input;
        }
        let state = &mut self.state;
        let mask = state.buffer.len() - 1;
        state.buffer[state.write] = input;
        state.valid = (state.valid + 1).min(state.buffer.len());
        let mut output = input;
        for ch in 0..2 {
            let delay = (state.delay[0] + state.delay[1] * state.sine[ch]).max(0.0);
            let whole = delay as usize;
            let fraction = delay - whole as f64;
            let a = if whole < state.valid {
                state.buffer[state.write.wrapping_sub(whole) & mask][ch]
            } else {
                0.0
            };
            let b = if whole + 1 < state.valid {
                state.buffer[state.write.wrapping_sub(whole + 1) & mask][ch]
            } else {
                0.0
            };
            let wet = a + (b - a) * fraction;
            output[ch] = input[ch] + (wet - input[ch]) * state.parameters.mix;
            // 更新済みsineを2行目へ使うmodified coupled form。
            state.sine[ch] += state.epsilon[ch] * state.cosine[ch];
            state.cosine[ch] -= state.epsilon[ch] * state.sine[ch];
        }
        state.write = (state.write + 1) & mask;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ChorusParameters {
        ChorusParameters {
            enabled: true,
            time_ms: 5.0,
            depth_ms: 0.0,
            rate_hz: 0.7,
            mix: 1.0,
        }
    }

    #[test]
    fn fractional_delay_preserves_each_channel() {
        let mut p = settings();
        p.time_ms = 5.01;
        let mut chorus = Chorus::new(p);
        assert!(chorus.prepare(48000.0));
        for i in 0..245 {
            let y = chorus.process_sample(if i == 0 { [1.0, -0.5] } else { [0.0; 2] });
            let expected = match i {
                240 => [0.52, -0.26],
                241 => [0.48, -0.24],
                _ => [0.0; 2],
            };
            for ch in 0..2 {
                assert!((y[ch] - expected[ch]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn bypass_and_reset_do_not_replay_history() {
        let mut p = settings();
        let mut chorus = Chorus::new(p);
        chorus.process_sample([1.0, -1.0]);
        p.enabled = false;
        chorus.set_parameters(p);
        assert_eq!(chorus.process_sample([0.5, -0.25]), [0.5, -0.25]);
        p.enabled = true;
        chorus.set_parameters(p);
        for _ in 0..300 {
            assert_eq!(chorus.process_sample([0.0; 2]), [0.0; 2]);
        }
        chorus.process_sample([1.0; 2]);
        chorus.reset();
        for _ in 0..300 {
            assert_eq!(chorus.process_sample([0.0; 2]), [0.0; 2]);
        }
    }

    #[test]
    fn rate_change_keeps_the_lfo_phase() {
        let mut p = settings();
        let mut chorus = Chorus::new(p);
        for _ in 0..10000 {
            chorus.process_sample([0.0; 2]);
        }
        let sine = chorus.state.sine;
        let cosine: [f64; 2] = std::array::from_fn(|i| {
            (chorus.state.cosine[i] + sine[i] * chorus.state.half[i][0]) / chorus.state.half[i][1]
        });
        p.rate_hz = 5.0;
        chorus.set_parameters(p);
        assert_eq!(chorus.state.sine, sine);
        for i in 0..2 {
            let actual = (chorus.state.cosine[i] + sine[i] * chorus.state.half[i][0])
                / chorus.state.half[i][1];
            assert!((actual - cosine[i]).abs() < 1e-14);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 長い数値応答の照合は実CPUで行う。
    fn rates_and_parameter_extremes_are_finite_and_bounded() {
        for fs in [44100.0, 48000.0, 96000.0, 192000.0] {
            let mut p = settings();
            p.depth_ms = 5.0;
            p.rate_hz = 5.0;
            let mut chorus = Chorus::new(p);
            assert!(chorus.prepare(fs));
            let capacity = chorus.state.buffer.capacity();
            for i in 0..20000 {
                let input = (i as f64 * 0.13).sin();
                let output = chorus.process_sample([input, -input]);
                assert!(output.into_iter().all(|x| x.is_finite() && x.abs() <= 1.0));
            }
            assert_eq!(chorus.state.buffer.capacity(), capacity);
        }
    }

    #[test]
    fn resource_limits_reject_invalid_setups() {
        let mut chorus = Chorus::new(settings());
        for fs in [0.0, 44099.0, f64::NAN, f64::INFINITY, 1e30] {
            assert!(!chorus.prepare(fs));
        }
        assert!(chorus.prepare(192000.0));
    }
}

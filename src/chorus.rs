// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 左右1tapの可変遅延。ChorusとFlangerを時間・変調幅・帰還の設定で連続的に扱う。

/// コントローラーで検証した定常設定。時間はms、RateはHz、Mixは0〜1。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChorusParameters {
    /// 遅延の中心（0.1〜40 ms）。
    pub time_ms: f64,
    /// 中心からの変調幅（0〜10 ms）。
    pub depth_ms: f64,
    /// 左右LFOの基準周波数（0.05〜5 Hz）。
    pub rate_hz: f64,
    /// 左をRate×(1+Detune)、右をRate×(1−Detune)にする比率（−0.1〜0.1）。
    pub detune: f64,
    /// 各チャンネルの遅延音を入力へ戻す倍率（−0.95〜0.95）。
    pub feedback: f64,
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
    /// epsilon/2とsqrt(1−(epsilon/2)^2)。Rate/Detune変更時の座標変換に使う。
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
        Self {
            state: ChorusState {
                buffer: Vec::new(),
                write: 0,
                valid: 0,
                sample_rate: 0.0,
                parameters,
                delay: [0.0; 2],
                sine: [0.0; 2],
                cosine: [1.0, -1.0],
                epsilon: [0.0; 2],
                half: [[0.0, 1.0]; 2],
            },
        }
    }

    /// setup専用。最大Time+Depthの履歴を確保し、失敗時は以前の状態を保つ。
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        let required = (sample_rate * 0.050).ceil() + 2.0;
        if !sample_rate.is_finite() || sample_rate < 44100.0 || required > Self::MAX_FRAMES as f64 {
            return false;
        }
        if self.prepared_for(sample_rate) {
            return true;
        }
        let Some(length) = (required as usize).checked_next_power_of_two() else {
            return false;
        };
        if length > Self::MAX_FRAMES {
            return false;
        }
        let mut buffer = Vec::new();
        if buffer.try_reserve_exact(length).is_err() {
            return false;
        }
        buffer.resize(length, [0.0; 2]);
        self.state.buffer = buffer;
        self.state.sample_rate = sample_rate;
        self.update_delay();
        self.update_rate();
        self.reset();
        true
    }

    pub fn prepared_for(&self, sample_rate: f64) -> bool {
        self.state.sample_rate == sample_rate && sample_rate >= 44100.0
    }

    pub fn set_parameters(&mut self, parameters: ChorusParameters) {
        if parameters == self.state.parameters {
            return;
        }
        let previous = self.state.parameters;
        self.state.parameters = parameters;
        if self.state.sample_rate == 0.0 {
            return;
        }
        if parameters.time_ms != previous.time_ms || parameters.depth_ms != previous.depth_ms {
            self.update_delay();
        }
        if parameters.rate_hz != previous.rate_hz || parameters.detune != previous.detune {
            self.update_rate();
        }
    }

    pub fn set_depth_ms(&mut self, depth_ms: f64) {
        self.state.parameters.depth_ms = depth_ms;
        if self.state.sample_rate != 0.0 {
            self.state.delay[1] =
                (depth_ms * self.state.sample_rate * 0.001).min(self.state.delay[0] - 1.0);
        }
    }

    fn update_delay(&mut self) {
        let p = self.state.parameters;
        let center = p.time_ms * self.state.sample_rate * 0.001;
        // 帰還が未確定の現在サンプルを参照しないよう、正弦変調の幅を制限する。
        self.state.delay = [
            center,
            (p.depth_ms * self.state.sample_rate * 0.001).min(center - 1.0),
        ];
    }

    fn update_rate(&mut self) {
        let p = self.state.parameters;
        for (i, ratio) in [1.0 + p.detune, 1.0 - p.detune].into_iter().enumerate() {
            let old = self.state.half[i];
            let physical_cosine = (self.state.cosine[i] + self.state.sine[i] * old[0]) / old[1];
            let h = (std::f64::consts::PI * p.rate_hz * ratio / self.state.sample_rate).sin();
            let g = (1.0 - h * h).sqrt();
            self.state.epsilon[i] = 2.0 * h;
            self.state.half[i] = [h, g];
            // Rate/Detune変更で保持した位相を変えず、新しいMCFの座標へ移す。
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
        debug_assert!(
            self.state.sample_rate >= 44100.0,
            "prepare before processing"
        );
        let state = &mut self.state;
        let mask = state.buffer.len() - 1;
        state.valid = (state.valid + 1).min(state.buffer.len());
        let mut output = input;
        for ch in 0..2 {
            let delay = (state.delay[0] + state.delay[1] * state.sine[ch]).max(1.0);
            // 準備時の上限は2^20 sample。u64相当のusizeとの浮動小数点変換を避ける。
            let whole = delay as u32;
            let fraction = delay - f64::from(whole);
            let whole = whole as usize;
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
            state.buffer[state.write][ch] = input[ch] + state.parameters.feedback * wet;
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
            time_ms: 20.0,
            depth_ms: 2.0,
            rate_hz: 0.7,
            detune: 0.0,
            feedback: 0.0,
            mix: 0.5,
        }
    }

    #[test]
    fn depth_update_matches_full_settings_with_live_history() {
        let mut p = settings();
        let mut full = Chorus::new(p);
        let mut depth = full.clone();
        depth.set_depth_ms(3.0);
        p.depth_ms = 3.0;
        full.set_parameters(p);
        assert_eq!(depth.state.parameters, full.state.parameters);
        for rate in [44100.0, 48000.0, 96000.0] {
            assert!(full.prepare(rate));
            assert!(depth.prepare(rate));
            for n in 0..10000 {
                if n % 37 == 0 {
                    p.depth_ms = (n % 101) as f64 * 0.1;
                    full.set_parameters(p);
                    depth.set_depth_ms(p.depth_ms);
                }
                let input = [(n as f64 * 0.13).sin(), (n as f64 * 0.37).cos()];
                assert_eq!(full.process_sample(input), depth.process_sample(input));
            }
        }
    }

    #[test]
    fn integer_delay_feedback_and_fractional_readout() {
        for feedback in [-0.95, 0.0, 0.5, 0.95] {
            let mut p = settings();
            p.time_ms = 1.0;
            p.depth_ms = 0.0;
            p.feedback = feedback;
            p.mix = 1.0;
            let mut chorus = Chorus::new(p);
            assert!(chorus.prepare(48000.0));
            for n in 0..1000 {
                let out = chorus.process_sample(if n == 0 { [1.0, -0.5] } else { [0.0; 2] });
                let expected = if n > 0 && n % 48 == 0 {
                    feedback.powi(n / 48 - 1)
                } else {
                    0.0
                };
                assert!((out[0] - expected).abs() < 1e-12);
                assert!((out[1] + expected * 0.5).abs() < 1e-12);
            }
        }
        let mut p = settings();
        p.time_ms = 5.01;
        p.depth_ms = 0.0;
        p.mix = 1.0;
        let mut chorus = Chorus::new(p);
        assert!(chorus.prepare(48000.0));
        for n in 0..500 {
            let out = chorus.process_sample(if n == 0 { [1.0, -0.5] } else { [0.0; 2] });
            let expected = match n {
                240 => 0.52,
                241 => 0.48,
                _ => 0.0,
            };
            assert!((out[0] - expected).abs() < 1e-12);
            assert!((out[1] + expected * 0.5).abs() < 1e-12);
        }
    }

    #[test]
    fn moving_delay_matches_sine_reference_with_rate_and_detune_automation() {
        for rate in [44100.0, 48000.0, 96000.0, 192000.0] {
            let mut p = settings();
            let mut chorus = Chorus::new(p);
            assert!(chorus.prepare(rate));
            let pointer = chorus.state.buffer.as_ptr();
            let mut history = vec![[0.0; 2]; 20000];
            let mut phase = [0.0, std::f64::consts::PI];
            let mut error = 0.0;
            let mut energy = 0.0;
            for n in 0..history.len() {
                if n % 257 == 0 {
                    let k = n / 257;
                    p.time_ms = [0.1, 2.5, 40.0][k % 3];
                    p.depth_ms = [10.0, 0.5, 5.0][k % 3];
                    p.rate_hz = [0.05, 0.7, 5.0][k % 3];
                    p.detune = [-0.1, 0.0, 0.1][k % 3];
                    p.feedback = [-0.95, 0.0, 0.95][k % 3];
                    p.mix = [0.0, 0.5, 1.0][k % 3];
                    chorus.set_parameters(p);
                }
                let input = [(n as f64 * 0.17).sin(), (n as f64 * 0.13).cos()];
                let output = chorus.process_sample(input);
                for ch in 0..2 {
                    let center = p.time_ms * rate * 0.001;
                    let depth = (p.depth_ms * rate * 0.001).min(center - 1.0);
                    let delay = center + depth * phase[ch].sin();
                    let whole = delay.floor() as usize;
                    let fraction = delay - whole as f64;
                    let a = n.checked_sub(whole).map_or(0.0, |i| history[i][ch]);
                    let b = n.checked_sub(whole + 1).map_or(0.0, |i| history[i][ch]);
                    let wet = a + (b - a) * fraction;
                    history[n][ch] = input[ch] + p.feedback * wet;
                    let expected = input[ch] * (1.0 - p.mix) + wet * p.mix;
                    error += (output[ch] - expected).powi(2);
                    energy += expected * expected;
                    assert!(output[ch].is_finite() && output[ch].abs() <= 20.0);
                    phase[ch] += std::f64::consts::TAU
                        * p.rate_hz
                        * (1.0 + if ch == 0 { p.detune } else { -p.detune })
                        / rate;
                }
            }
            assert!(
                error / energy < 1e-8,
                "rate={rate}, relative RMS={}",
                (error / energy).sqrt()
            );
            assert_eq!(pointer, chorus.state.buffer.as_ptr());
            chorus.reset();
            for _ in 0..10000 {
                assert_eq!(chorus.process_sample([0.0; 2]), [0.0; 2]);
            }
        }
    }

    #[test]
    fn setup_failure_keeps_history_and_limits_resources() {
        let p = settings();
        let mut chorus = Chorus::new(p);
        chorus.set_parameters(p);
        assert!(chorus.prepare(48000.0));
        for _ in 0..2000 {
            chorus.process_sample([1.0, -1.0]);
        }
        let mut expected = chorus.clone();
        for rate in [0.0, 44099.0, f64::NAN, f64::INFINITY, 1e30] {
            assert!(!chorus.prepare(rate));
        }
        for _ in 0..5000 {
            assert_eq!(
                chorus.process_sample([0.0; 2]),
                expected.process_sample([0.0; 2])
            );
        }
        assert!(chorus.prepare(192000.0));
        assert!(chorus.state.buffer.len() * 16 <= 16 * 1024 * 1024);
    }
}

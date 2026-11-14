// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 8本の遅延線をHouseholder行列で結合するステレオFDN。

use std::f64::consts::TAU;

const LENGTHS: [usize; 8] = [1493, 1747, 1999, 2287, 2591, 2927, 3301, 3917];
const SCALE: f64 = 0.353_553_390_593_273_8;
const INPUT_RIGHT: [f64; 8] = [1., -1., 1., -1., 1., -1., 1., -1.];
const OUTPUT_LEFT: [f64; 8] = [1., 1., -1., -1., 1., 1., -1., -1.];
const OUTPUT_RIGHT: [f64; 8] = [1., -1., -1., 1., 1., -1., -1., 1.];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParameters {
    /// 低域の設計残響時間（秒）。
    pub decay_seconds: f64,
    /// 帰還1極LPFの設計周波数（Hz）。
    pub damping_hz: f64,
    /// 遅延長の倍率（0.5〜2）。変更時は尾部をリセットする。
    pub size: f64,
    /// Dry/Wet線形混合量（0〜1）。
    pub mix: f64,
}

#[cfg_attr(test, derive(Clone))]
struct ReverbState {
    /// 最大Sizeの領域を準備時に確保する。
    lines: [Vec<f64>; 8],
    /// 各遅延線の現在の長さと読書き位置。
    lengths: [usize; 8],
    /// 次に読書きする各線の位置。
    cursors: [usize; 8],
    /// リセット前のバッファを読まないための充填待ちサンプル数。
    warmup: [usize; 8],
    /// 帰還LPFの履歴と、遅延長別のT60利得。
    low: [f64; 8],
    /// 遅延長ごとの低域減衰倍率。
    gains: [f64; 8],
    /// 1極LPFの更新係数。
    damping: f64,
    /// 適用中の設定とレート。
    parameters: ReverbParameters,
    /// 準備済みのサンプルレート（Hz）。
    sample_rate: f64,
}

#[cfg_attr(test, derive(Clone))]
pub struct Reverb {
    /// 専用の履歴・係数・設定。
    state: ReverbState,
}

impl Reverb {
    /// 8本のf64履歴を合計16 MiB以内に制限する。
    const MAX_LINE_SAMPLES: usize = 1 << 18;

    pub fn new(parameters: ReverbParameters) -> Self {
        Self {
            state: ReverbState {
                lines: std::array::from_fn(|_| Vec::new()),
                lengths: [1; 8],
                cursors: [0; 8],
                warmup: [1; 8],
                low: [0.; 8],
                gains: [0.; 8],
                damping: 0.,
                parameters,
                sample_rate: 0.,
            },
        }
    }

    /// setupProcessing専用。確保不能なら以前の設定を維持する。
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        if !sample_rate.is_finite() || sample_rate < 44100. {
            return false;
        }
        if self.state.sample_rate == sample_rate {
            return true;
        }
        let mut lines = std::array::from_fn(|_| Vec::new());
        for (line, base) in lines.iter_mut().zip(LENGTHS) {
            let length = (base as f64 * sample_rate / 48000. * 2.).ceil();
            if !length.is_finite() || length > Self::MAX_LINE_SAMPLES as f64 {
                return false;
            }
            let length = length as usize;
            if line.try_reserve_exact(length).is_err() {
                return false;
            }
            line.resize(length, 0.);
        }
        self.state.lines = lines;
        self.state.sample_rate = sample_rate;
        self.update_lengths();
        self.update_decay();
        self.update_damping();
        self.reset();
        true
    }

    pub fn prepared_for(&self, sample_rate: f64) -> bool {
        self.state.sample_rate == sample_rate && sample_rate >= 44100.0
    }

    pub fn set_mix(&mut self, mix: f64) {
        self.state.parameters.mix = mix;
    }

    pub fn set_parameters(&mut self, parameters: ReverbParameters) {
        let old = self.state.parameters;
        self.state.parameters = parameters;
        if self.state.sample_rate == 0.0 {
            return;
        }
        if old.size != parameters.size {
            self.update_lengths();
        }
        if old.size != parameters.size || old.decay_seconds != parameters.decay_seconds {
            self.update_decay();
        }
        if old.damping_hz != parameters.damping_hz {
            self.update_damping();
        }
        if old.size != parameters.size {
            self.reset();
        }
    }

    fn update_lengths(&mut self) {
        for (i, base) in LENGTHS.into_iter().enumerate() {
            self.state.lengths[i] = ((base as f64 * self.state.sample_rate / 48000.
                * self.state.parameters.size)
                .round() as usize)
                .clamp(1, self.state.lines[i].len());
        }
    }

    fn update_decay(&mut self) {
        for (gain, length) in self.state.gains.iter_mut().zip(self.state.lengths) {
            *gain = 10_f64.powf(
                -3. * length as f64
                    / (self.state.sample_rate * self.state.parameters.decay_seconds),
            );
        }
    }

    fn update_damping(&mut self) {
        self.state.damping =
            1. - (-TAU * self.state.parameters.damping_hz / self.state.sample_rate).exp();
    }

    /// 大きい領域を走査せず、各線が再充填されるまで読み出しを無音にする。
    pub fn reset(&mut self) {
        self.state.cursors = [0; 8];
        self.state.warmup = self.state.lengths;
        self.state.low = [0.; 8];
    }

    pub fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        debug_assert!(
            self.state.sample_rate >= 44100.0,
            "prepare before processing"
        );
        let mut feedback = [0.; 8];
        let mut wet = [0.; 2];
        for (i, value) in feedback.iter_mut().enumerate() {
            let delayed = if self.state.warmup[i] == 0 {
                self.state.lines[i][self.state.cursors[i]]
            } else {
                self.state.warmup[i] -= 1;
                0.
            };
            wet[0] += OUTPUT_LEFT[i] * delayed;
            wet[1] += OUTPUT_RIGHT[i] * delayed;
            self.state.low[i] += self.state.damping * (delayed - self.state.low[i]);
            *value = self.state.low[i] * self.state.gains[i];
        }
        let reflected = feedback.iter().sum::<f64>() * 0.25;
        for (i, value) in feedback.into_iter().enumerate() {
            self.state.lines[i][self.state.cursors[i]] =
                (input[0] + INPUT_RIGHT[i] * input[1]) * SCALE + value - reflected;
            self.state.cursors[i] += 1;
            if self.state.cursors[i] == self.state.lengths[i] {
                self.state.cursors[i] = 0;
            }
        }
        let mix = self.state.parameters.mix;
        std::array::from_fn(|ch| input[ch] * (1. - mix) + wet[ch] * SCALE * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> ReverbParameters {
        ReverbParameters {
            decay_seconds: 2.,
            damping_hz: 6000.,
            size: 1.,
            mix: 1.,
        }
    }
    #[test]
    fn impulse_has_stereo_tail_and_reset_removes_old_history() {
        for rate in [44100., 48000., 96000.] {
            let mut reverb = Reverb::new(settings());
            assert!(reverb.prepare(rate));
            let mut energy = [0.; 2];
            for n in 0..rate as usize * 3 {
                let out = reverb.process_sample(if n == 0 { [1., -1.] } else { [0.; 2] });
                assert!(out.into_iter().all(f64::is_finite));
                for ch in 0..2 {
                    energy[ch] += out[ch] * out[ch];
                }
            }
            assert!(energy.into_iter().all(|x| x > 0.01));
            reverb.reset();
            for _ in 0..rate as usize {
                assert_eq!(reverb.process_sample([0.; 2]), [0.; 2]);
            }
        }
    }
    #[test]
    fn mix_and_size_changes_keep_the_buffer_allocation() {
        let mut p = settings();
        let mut reverb = Reverb::new(p);
        assert!(reverb.prepare(48000.0));
        let pointers = reverb.state.lines.each_ref().map(|v| v.as_ptr());
        for size in [0.5, 2., 1.] {
            p.size = size;
            reverb.set_parameters(p);
            assert_eq!(pointers, reverb.state.lines.each_ref().map(|v| v.as_ptr()));
            for _ in 0..10000 {
                assert_eq!(reverb.process_sample([0.; 2]), [0.; 2]);
            }
        }
        p.mix = 0.;
        reverb.set_parameters(p);
        assert_eq!(reverb.process_sample([0.25, -0.5]), [0.25, -0.5]);
    }

    #[test]
    fn settings_before_preparation_and_size_change_discard_old_tail() {
        let mut p = settings();
        let mut reverb = Reverb::new(p);
        p.size = 2.0;
        p.decay_seconds = 10.0;
        reverb.set_parameters(p);
        assert!(reverb.prepare(48000.0));
        let pointers = reverb.state.lines.each_ref().map(|v| v.as_ptr());
        for _ in 0..10000 {
            reverb.process_sample([0.5, -0.25]);
        }
        p.size = 0.5;
        reverb.set_parameters(p);
        assert_eq!(pointers, reverb.state.lines.each_ref().map(|v| v.as_ptr()));
        for _ in 0..48000 {
            assert_eq!(reverb.process_sample([0.0; 2]), [0.0; 2]);
        }
    }

    #[test]
    fn preparation_bounds_resources_and_keeps_state_on_failure() {
        let mut reverb = Reverb::new(settings());
        assert!(reverb.prepare(48000.0));
        reverb.process_sample([1., -1.]);
        let pointers = reverb.state.lines.each_ref().map(|v| v.as_ptr());
        let mut reference = reverb.clone();
        for rate in [0., 44099., f64::NAN, f64::INFINITY, 1e9, f64::MAX] {
            assert!(!reverb.prepare(rate));
            assert_eq!(reverb.state.sample_rate, 48000.);
            assert_eq!(pointers, reverb.state.lines.each_ref().map(|v| v.as_ptr()));
        }
        for _ in 0..10000 {
            assert_eq!(
                reverb.process_sample([0.; 2]),
                reference.process_sample([0.; 2])
            );
        }
        assert!(reverb.prepare(384000.));
        let bytes = reverb
            .state
            .lines
            .iter()
            .map(|v| v.len() * 8)
            .sum::<usize>();
        assert!(bytes <= 16 * 1024 * 1024);
    }
}

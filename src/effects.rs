// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! Voice合成後、EQ前に置く3つの独立した直列エフェクタ。

use crate::chorus::{Chorus, ChorusParameters};
use crate::delay::{Delay, DelayParameters};
use crate::distortion::{ClipperType, Distortion, DistortionParameters};
use crate::parameters::{self, EFFECT_SLOTS};
use crate::reverb::{Reverb, ReverbParameters};

/// 各Distortionの1サンプル遅延を含む、系列全体の固定遅延。
pub const LATENCY: usize = 3 * crate::distortion::LATENCY;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectType {
    Off,
    Distortion,
    Delay,
    Reverb,
    Chorus,
}

#[cfg_attr(test, derive(Clone))]
struct EffectState {
    /// Offではこのスロットをそのまま通過する。
    kind: EffectType,
    /// テンポ同期ディレイの設定と左右の履歴。
    delay: Delay,
    /// スロットごとに独立した8本のFDN履歴。
    reverb: Reverb,
    /// 可変遅延・帰還・左右LFOの独立した履歴。
    chorus: Chorus,
    /// このスロットだけの設定と左右独立のADAA履歴。
    parameters: DistortionParameters,
    distortion: Distortion,
    /// 適用中のClipper。設定との差をフェード付きで切り替える。
    processing: ClipperType,
    /// fade-out 32、履歴充填2、fade-in 32サンプルの残り。
    transition: u8,
    /// 処理開始前のClipper変更は即時反映する。
    started: bool,
    /// 平均化を掛けないDryの1サンプル純遅延。
    previous_dry: [f64; 2],
}

#[cfg_attr(test, derive(Clone))]
struct Effect {
    state: EffectState,
}

impl Effect {
    fn new(
        parameters: DistortionParameters,
        delay_parameters: DelayParameters,
        reverb_parameters: ReverbParameters,
        chorus_parameters: ChorusParameters,
    ) -> Self {
        Self {
            state: EffectState {
                kind: EffectType::Off,
                delay: Delay::new(delay_parameters),
                reverb: Reverb::new(reverb_parameters),
                chorus: Chorus::new(chorus_parameters),
                parameters,
                distortion: Distortion::new(),
                processing: parameters.clipper,
                transition: 0,
                started: false,
                previous_dry: [0.0; 2],
            },
        }
    }

    fn set_parameters(
        &mut self,
        kind: EffectType,
        parameters: DistortionParameters,
        delay_parameters: DelayParameters,
    ) {
        let changed = kind != self.state.kind;
        self.state.parameters = parameters;
        self.state.kind = kind;
        self.state.delay.set_parameters(delay_parameters);
        if changed || !self.state.started {
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.state.distortion.reset();
        self.state.delay.reset();
        self.state.reverb.reset();
        self.state.chorus.reset();
        self.state.previous_dry = [0.0; 2];
        self.state.processing = self.state.parameters.clipper;
        self.state.transition = 0;
        self.state.started = false;
    }

    fn process_sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        self.state.started = true;
        if self.state.transition == 0 && self.state.processing != self.state.parameters.clipper {
            self.state.transition = 66;
        }
        if self.state.transition == 34 {
            self.state.processing = self.state.parameters.clipper;
            self.state.distortion.reset();
        }
        let remaining = self.state.transition;
        self.state.transition = remaining.saturating_sub(1);
        let dry = std::mem::replace(&mut self.state.previous_dry, input);
        if self.state.processing == ClipperType::None {
            return dry;
        }
        let mut parameters = self.state.parameters;
        parameters.clipper = self.state.processing;
        let wet = self.state.distortion.process_sample(input, &parameters);
        let fade = match remaining {
            0 => 1.0,
            1..=32 => f64::from(32 - remaining) / 32.0,
            33..=34 => 0.0,
            _ => f64::from(remaining - 35) / 32.0,
        };
        std::array::from_fn(|ch| dry[ch] * (1.0 - parameters.mix) + wet[ch] * parameters.mix * fade)
    }

    fn process_samples(&mut self, samples: &mut [[f64; 2]]) {
        if samples.is_empty() {
            return;
        }
        match self.state.kind {
            EffectType::Off => return,
            EffectType::Delay => {
                for sample in samples {
                    *sample = self.state.delay.process_sample(*sample);
                }
                self.state.started = true;
                return;
            }
            EffectType::Reverb => {
                for sample in samples {
                    *sample = self.state.reverb.process_sample(*sample);
                }
                self.state.started = true;
                return;
            }
            EffectType::Chorus => {
                for sample in samples {
                    *sample = self.state.chorus.process_sample(*sample);
                }
                self.state.started = true;
                return;
            }
            EffectType::Distortion => {}
        }
        if self.state.transition != 0 || self.state.processing != self.state.parameters.clipper {
            for sample in samples {
                *sample = self.process_sample(*sample);
            }
            return;
        }
        self.state.started = true;
        if self.state.processing == ClipperType::None {
            for sample in samples {
                std::mem::swap(sample, &mut self.state.previous_dry);
            }
            return;
        }
        let parameters = self.state.parameters;
        for chunk in samples.chunks_mut(32) {
            let mut wet = [[0.0; 2]; 32];
            self.state
                .distortion
                .process_samples(chunk, &mut wet[..chunk.len()], &parameters);
            for (sample, wet) in chunk.iter_mut().zip(wet) {
                let dry = std::mem::replace(&mut self.state.previous_dry, *sample);
                *sample = std::array::from_fn(|ch| {
                    dry[ch] * (1.0 - parameters.mix) + wet[ch] * parameters.mix
                });
            }
        }
    }
}

#[cfg_attr(test, derive(Clone))]
struct EffectsState {
    /// 上から順に処理する固定3スロット。履歴は交換可能な設定とは分離する。
    slots: [Effect; 3],
    /// 有効なDistortion数に応じて補う純遅延の長さ。
    padding: usize,
    /// 最後に受信した有効なホストBPM。取得前は120。
    tempo: f64,
    /// 全スロットOffでも全体を3サンプル遅らせるための固定領域。
    delay: [[f64; 2]; LATENCY],
}

#[cfg_attr(test, derive(Clone))]
pub struct Effects {
    state: EffectsState,
}

impl Effects {
    pub fn new() -> Self {
        let values = parameters::defaults();
        Self {
            state: EffectsState {
                slots: std::array::from_fn(|slot| {
                    Effect::new(
                        parameters::distortion(&values, slot),
                        parameters::delay(&values, slot),
                        parameters::reverb(&values, slot),
                        parameters::chorus(&values, slot),
                    )
                }),
                padding: LATENCY,
                tempo: 120.0,
                delay: [[0.0; 2]; LATENCY],
            },
        }
    }

    /// 全スロットのリングを停止中に準備する。全確保が成功してから置き換える。
    pub fn prepare(&mut self, sample_rate: f64, values: &[f64; parameters::COUNT]) -> bool {
        if self.state.slots[0].state.delay.prepared_for(sample_rate)
            && self.state.slots[0].state.reverb.prepared_for(sample_rate)
            && self.state.slots[0].state.chorus.prepared_for(sample_rate)
        {
            self.reset();
            return true;
        }
        let mut delays: [Delay; 3] =
            std::array::from_fn(|slot| Delay::new(parameters::delay(values, slot)));
        let mut reverbs: [Reverb; 3] =
            std::array::from_fn(|slot| Reverb::new(parameters::reverb(values, slot)));
        let mut choruses: [Chorus; 3] =
            std::array::from_fn(|slot| Chorus::new(parameters::chorus(values, slot)));
        for ((delay, reverb), chorus) in delays.iter_mut().zip(&mut reverbs).zip(&mut choruses) {
            if !delay.prepare(sample_rate)
                || !reverb.prepare(sample_rate)
                || !chorus.prepare(sample_rate)
            {
                return false;
            }
            delay.set_tempo(self.state.tempo);
        }
        for (((slot, delay), reverb), chorus) in self
            .state
            .slots
            .iter_mut()
            .zip(delays)
            .zip(reverbs)
            .zip(choruses)
        {
            slot.state.delay = delay;
            slot.state.reverb = reverb;
            slot.state.chorus = chorus;
        }
        true
    }

    pub fn set_tempo(&mut self, tempo: f64) {
        if tempo.is_finite() && tempo > 0.0 && tempo != self.state.tempo {
            self.state.tempo = tempo;
            for slot in &mut self.state.slots {
                slot.state.delay.set_tempo(tempo);
            }
        }
    }

    pub fn set_parameters(
        &mut self,
        values: &[f64; parameters::COUNT],
        previous: &[f64; parameters::COUNT],
    ) {
        for (slot, indices) in EFFECT_SLOTS.into_iter().enumerate() {
            if indices.into_iter().any(|i| values[i] != previous[i]) {
                if indices[8..12].iter().any(|&i| values[i] != previous[i]) {
                    self.state.slots[slot]
                        .state
                        .reverb
                        .set_parameters(parameters::reverb(values, slot));
                }
                if indices[12..].iter().any(|&i| values[i] != previous[i]) {
                    self.state.slots[slot]
                        .state
                        .chorus
                        .set_parameters(parameters::chorus(values, slot));
                }
                // 他のエフェクト設定変更でDistortionのGain換算をやり直さない。
                let distortion = if indices[1..4].iter().any(|&i| values[i] != previous[i]) {
                    parameters::distortion(values, slot)
                } else {
                    self.state.slots[slot].state.parameters
                };
                self.state.slots[slot].set_parameters(
                    parameters::EFFECT_TYPES
                        [parameters::to_plain(indices[0], values[indices[0]]) as usize],
                    distortion,
                    parameters::delay(values, slot),
                );
            }
        }
        let padding = self
            .state
            .slots
            .iter()
            .filter(|slot| slot.state.kind != EffectType::Distortion)
            .count();
        if padding != self.state.padding {
            self.state.padding = padding;
            self.state.delay.fill([0.0; 2]);
        }
    }

    pub fn reset(&mut self) {
        for slot in &mut self.state.slots {
            slot.reset();
        }
        self.state.delay.fill([0.0; 2]);
    }

    pub fn process_sample(&mut self, mut sample: [f64; 2]) -> [f64; 2] {
        for slot in &mut self.state.slots {
            sample = match slot.state.kind {
                EffectType::Off => sample,
                EffectType::Distortion => slot.process_sample(sample),
                EffectType::Delay => {
                    slot.state.started = true;
                    slot.state.delay.process_sample(sample)
                }
                EffectType::Chorus => {
                    slot.state.started = true;
                    slot.state.chorus.process_sample(sample)
                }
                EffectType::Reverb => {
                    slot.state.started = true;
                    slot.state.reverb.process_sample(sample)
                }
            };
        }
        self.delay_sample(sample)
    }

    pub fn process_samples(&mut self, samples: &mut [[f64; 2]]) {
        for slot in &mut self.state.slots {
            slot.process_samples(samples);
        }
        for sample in samples {
            *sample = self.delay_sample(*sample);
        }
    }

    #[inline]
    fn delay_sample(&mut self, mut sample: [f64; 2]) -> [f64; 2] {
        for delayed in &mut self.state.delay[..self.state.padding] {
            std::mem::swap(&mut sample, delayed);
        }
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(mask: usize) -> ([f64; parameters::COUNT], Effects) {
        let mut values = parameters::defaults();
        for (slot, [kind, clipper, gain, mix, ..]) in EFFECT_SLOTS.into_iter().enumerate() {
            values[kind] = if mask & (1 << slot) != 0 {
                1.0 / 4.0
            } else {
                0.0
            };
            values[clipper] = parameters::to_normalized(clipper, if slot == 1 { 2.0 } else { 1.0 });
            values[gain] = parameters::to_normalized(gain, 6.0 + slot as f64 * 9.0);
            values[mix] = 0.3 + slot as f64 * 0.2;
        }
        let mut effects = Effects::new();
        effects.set_parameters(&values, &parameters::defaults());
        (values, effects)
    }

    #[test]
    fn independent_slots_follow_order_with_exact_dry_latency() {
        for mask in 0..8 {
            let (values, mut effects) = configured(mask);
            let mut wet: [Distortion; 3] = std::array::from_fn(|_| Distortion::new());
            let mut dry = [[0.0; 2]; 3];
            let count = mask.count_ones() as usize;
            let mut delay = std::collections::VecDeque::from(vec![[0.0; 2]; LATENCY - count]);
            for n in 0..300 {
                let input = [(n as f64 * 0.19).sin(), (n as f64 * 0.071).cos() * 0.4];
                let mut expected = input;
                for slot in 0..3 {
                    if mask & (1 << slot) == 0 {
                        continue;
                    }
                    let p = parameters::distortion(&values, slot);
                    let processed = wet[slot].process_sample(expected, &p);
                    let previous = std::mem::replace(&mut dry[slot], expected);
                    expected = std::array::from_fn(|ch| {
                        previous[ch] * (1.0 - p.mix) + processed[ch] * p.mix
                    });
                }
                delay.push_back(expected);
                assert_eq!(effects.process_sample(input), delay.pop_front().unwrap());
            }
            effects.reset();
            for _ in 0..16 {
                assert_eq!(effects.process_sample([0.0; 2]), [0.0; 2]);
            }
        }
    }

    #[test]
    fn off_skips_processing_and_all_dry_paths_have_three_sample_latency() {
        for mask in 0..8 {
            let (mut p, mut effects) = configured(mask);
            let previous = p;
            for [_, _, _, mix, ..] in EFFECT_SLOTS {
                p[mix] = 0.0;
            }
            effects.set_parameters(&p, &previous);
            for n in 0..16 {
                let input = if n == 0 { [0.75, -0.25] } else { [0.0; 2] };
                let expected = if n == LATENCY {
                    [0.75, -0.25]
                } else {
                    [0.0; 2]
                };
                assert_eq!(effects.process_sample(input), expected);
            }
            for (i, slot) in effects.state.slots.iter().enumerate() {
                assert_eq!(slot.state.started, mask & (1 << i) != 0);
            }
        }
    }

    #[test]
    fn blocks_match_samples_during_selection_and_clipper_changes() {
        let (mut p, mut scalar) = configured(7);
        let mut blocks = scalar.clone();
        for step in 0..24 {
            let previous = p;
            let [kind, clipper, gain, mix, ..] = EFFECT_SLOTS[step % 3];
            p[kind] = if step % 4 == 0 { 0.0 } else { 1.0 / 4.0 };
            p[clipper] = (step % 3) as f64 / 2.0;
            p[gain] = (step % 7) as f64 / 6.0;
            p[mix] = (step % 5) as f64 / 4.0;
            scalar.set_parameters(&p, &previous);
            blocks.set_parameters(&p, &previous);
            let len = [0, 1, 2, 31, 32, 33, 127, 256][step % 8];
            let mut samples: Vec<_> = (0..len)
                .map(|n| [(n as f64 * 0.17).sin(), (n as f64 * 0.19).cos()])
                .collect();
            let expected: Vec<_> = samples.iter().map(|&x| scalar.process_sample(x)).collect();
            blocks.process_samples(&mut samples);
            assert_eq!(samples, expected, "step={step}");
        }
    }

    #[test]
    fn delay_slots_match_scalar_across_tempo_kind_and_mix_changes() {
        let (mut p, mut scalar) = configured(7);
        assert!(scalar.prepare(48000.0, &p));
        let mut blocks = scalar.clone();
        for step in 0..30 {
            let previous = p;
            let [kind, _, _, _, beat, feedback, mix, stereo, ..] = EFFECT_SLOTS[step % 3];
            p[kind] = parameters::to_normalized(kind, ((step / 3) % 5) as f64);
            p[stereo] = (step % 4) as f64 / 3.0;
            p[beat] = (step % 14) as f64 / 13.0;
            p[feedback] = (step % 6) as f64 / 5.0;
            p[mix] = (step % 5) as f64 / 4.0;
            for effects in [&mut scalar, &mut blocks] {
                effects.set_parameters(&p, &previous);
                effects.set_tempo(120.0 + step as f64 * 2.0);
            }
            let mut samples: Vec<_> = (0..1024)
                .map(|n| [(n as f64 * 0.17).sin(), (n as f64 * 0.19).cos()])
                .collect();
            let expected: Vec<_> = samples.iter().map(|&x| scalar.process_sample(x)).collect();
            blocks.process_samples(&mut samples);
            assert_eq!(samples, expected);
        }
        let previous = p;
        for [kind, ..] in EFFECT_SLOTS {
            p[kind] = 0.0;
        }
        scalar.set_parameters(&p, &previous);
        scalar.process_samples(&mut [[0.0; 2]; 32]);
        let previous = p;
        for [kind, ..] in EFFECT_SLOTS {
            p[kind] = parameters::to_normalized(kind, 2.0);
        }
        scalar.set_parameters(&p, &previous);
        for _ in 0..24000 {
            assert_eq!(scalar.process_sample([0.0; 2]), [0.0; 2]);
        }
    }

    #[test]
    fn swapping_adjacent_settings_changes_the_processing_order() {
        let (mut p, mut original) = configured(3);
        let previous = p;
        parameters::swap_effects(&mut p, 0);
        let mut reversed = Effects::new();
        reversed.set_parameters(&p, &parameters::defaults());
        let mut difference = 0.0;
        for n in 0..200 {
            let input = [(n as f64 * 0.1).sin(); 2];
            difference +=
                (original.process_sample(input)[0] - reversed.process_sample(input)[0]).abs();
        }
        assert!(difference > 1.0);
        parameters::swap_effects(&mut p, 0);
        assert_eq!(p, previous);
    }

    #[test]
    fn clipper_transitions_finish_and_do_not_reset_dry() {
        let p = DistortionParameters {
            clipper: ClipperType::None,
            gain: 4.0,
            mix: 0.0,
        };
        let delay_parameters = parameters::delay(&parameters::defaults(), 0);
        let mut effect = Effect::new(
            p,
            delay_parameters,
            parameters::reverb(&parameters::defaults(), 0),
            parameters::chorus(&parameters::defaults(), 0),
        );
        effect.set_parameters(EffectType::Distortion, p, delay_parameters);
        let mut dry = [0.0; 2];
        for clipper in [ClipperType::Soft, ClipperType::Hard, ClipperType::None] {
            effect.set_parameters(
                EffectType::Distortion,
                DistortionParameters { clipper, ..p },
                delay_parameters,
            );
            for n in 0..100 {
                let input = [n as f64 / 100.0, -(n as f64) / 100.0];
                assert_eq!(effect.process_sample(input), dry);
                dry = input;
            }
            assert_eq!(effect.state.processing, clipper);
            assert_eq!(effect.state.transition, 0);
        }
    }
}

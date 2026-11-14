// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::effects::Effects;
use crate::envelope::DEFAULT_ENVELOPE;
use crate::eq::Eq;
use crate::limiter::Limiter;
use crate::oscillator::{PanningGains, panning_gains};
use crate::parameters;
#[cfg(windows)]
use crate::voice::CutoffRatioCache;
use crate::voice::{Note, Voice};

#[cfg_attr(test, derive(Clone))]
pub struct SynthesizerState {
    /// 最大32枠を初期化時に確保し、音声処理中は確保・解放しない。
    voices: [Voice; 32],
    /// 次の有効なNote Onを割り当てるオシレータID。初期値0、使用枠数で循環する。
    next_oscillator_id: usize,
    /// 現在使用する枠数（1..=32）。範囲外の枠は停止し、位相だけ保持する。
    polyphony: usize,
    /// 停止後に一度でもNote Onを割り当てた枠の上端。未使用枠のサンプル処理を省く。
    used_voices: usize,
    /// 全オシレータに共通のサンプルレート（Hz）。
    sample_rate: f64,
    /// 適用済みの正規化値。変更時だけ実単位へ換算する。
    parameters: [f64; parameters::COUNT],
    /// ホストからの共通ピッチベンド（0〜1、中央0.5）。音色の保存対象ではない。
    pitch_bend: f64,
    /// 共有ディストーション・EQの後に掛ける振幅倍率。Volume変更時だけ再計算する。
    master_gain: f64,
    /// 全Voice共通の定位係数。実発音数・有効なWidth・Panの変更時だけ更新する。
    panning_gains: PanningGains,
    /// ユニゾンを使わないOsc2/3の共通Pan係数。
    mono_gains: [f64; 2],
    /// 停止・使用範囲拡大後の未使用枠を次のサンプルで更新する。
    unused_pending: bool,
    /// Voice合成後の独立した3スロット。
    effects: Effects,
    /// エフェクタ系列の後、Master前のEQ。
    eq: Eq,
    /// EQ後、Master Volume直前の固定5 ms先読みリミッター。
    limiter: Limiter,
}

/// Synthesizerの処理を担当し、専用のStateを所有するコンポーネント。
#[cfg_attr(test, derive(Clone))]
pub struct Synthesizer {
    /// このコンポーネントの内部状態。
    state: Box<SynthesizerState>,
}

impl Synthesizer {
    pub fn new() -> Self {
        Self {
            state: Box::new(SynthesizerState {
                voices: std::array::from_fn(|_| Voice::new()),
                next_oscillator_id: 0,
                polyphony: 1,
                used_voices: 0,
                sample_rate: 44100.0,
                parameters: parameters::defaults(),
                pitch_bend: 0.5,
                master_gain: 1.0,
                panning_gains: [[1.0; 2]; 7],
                mono_gains: [1.0; 2],
                unused_pending: false,
                effects: Effects::new(),
                eq: Eq::new(44100.0, parameters::eq(&parameters::defaults())),
                limiter: Limiter::new(),
            }),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) -> bool {
        if sample_rate.is_finite()
            && sample_rate >= crate::filter::MIN_SAMPLE_RATE
            && (!self
                .state
                .effects
                .prepare(sample_rate, &self.state.parameters)
                || !self.state.eq.prepare(sample_rate)
                || !self.state.limiter.prepare(sample_rate))
        {
            return false;
        }
        self.state.sample_rate = sample_rate;
        true
    }

    pub fn latency_samples(&self) -> usize {
        crate::effects::LATENCY + self.state.limiter.latency_samples()
    }

    pub fn set_tempo(&mut self, tempo: f64) {
        self.state.effects.set_tempo(tempo);
    }

    pub fn parameters(&self) -> [f64; parameters::COUNT] {
        self.state.parameters
    }

    pub fn pitch_bend(&self) -> f64 {
        self.state.pitch_bend
    }

    pub fn set_pitch_bend(&mut self, value: f64) {
        if self.state.pitch_bend != value {
            self.state.pitch_bend = value;
            let cents =
                parameters::to_plain(parameters::PITCH, self.state.parameters[parameters::PITCH]);
            let ratio = ((value * 2.0 - 1.0) * cents / 1200.0).exp2();
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_pitch_ratio(ratio);
            }
        }
    }

    /// 設定は呼出し中だけ借用し、有効な枠へ反映する。Polyphony増加時は追加枠も同期する。
    pub fn set_parameters(&mut self, values: &[f64; parameters::COUNT]) -> bool {
        let changes = parameters::ParameterChanges::new(values, &self.state.parameters);
        self.apply_parameters(values, changes)
    }

    /// キューが更新した項目だけ比較する。候補外の設定は既存値から変えないこと。
    pub fn set_automated_parameters(
        &mut self,
        values: &[f64; parameters::COUNT],
        candidates: [u64; parameters::COUNT.div_ceil(64)],
    ) -> bool {
        let changes = parameters::ParameterChanges::from_candidates(
            values,
            &self.state.parameters,
            candidates,
        );
        self.apply_parameters(values, changes)
    }

    fn apply_parameters(
        &mut self,
        values: &[f64; parameters::COUNT],
        changes: parameters::ParameterChanges,
    ) -> bool {
        if !changes.any(0, parameters::COUNT) {
            return false;
        }
        if changes.contains(13) {
            let db = parameters::to_plain(13, values[13]);
            #[cfg(windows)]
            let gain = (db * (std::f64::consts::LN_10 / 20.0)).exp();
            #[cfg(not(windows))]
            let gain = (db * (std::f64::consts::LOG2_10 / 20.0)).exp2();
            self.state.master_gain = gain;
            // 全体の音量だけが変わる場合は、各Voiceへの設定配送を省く。
            if !changes.any(0, 13) && !changes.any(14, parameters::COUNT) {
                self.save_parameters(values, &changes);
                return true;
            }
        }
        if changes.any(34, 42) {
            let mut eq = self.state.eq.parameters();
            eq.enabled = values[34] >= 0.5;
            for (indices, fields) in [
                ([35, 37, 40], &mut eq.frequencies_hz),
                ([36, 38, 41], &mut eq.gains_db),
            ] {
                for (i, field) in indices.into_iter().zip(fields) {
                    if changes.contains(i) {
                        *field = parameters::to_plain(i, values[i]);
                    }
                }
            }
            if changes.contains(39) {
                eq.mid_q = parameters::to_plain(39, values[39]);
            }
            self.state.eq.set_parameters(eq);
        }
        self.state.limiter.set_enabled(values[108] >= 0.5);
        if changes.any(31, 34) || changes.any(42, 130) {
            self.state.effects.set_parameters(values, &changes);
        }
        // Voiceへ渡す入力が同じなら、合成後の処理だけを更新して終える。
        if !changes.any(0, 13) && !changes.any(14, 31) && !changes.any(130, parameters::COUNT) {
            self.save_parameters(values, &changes);
            return true;
        }
        let previous = &self.state.parameters;
        // 有効範囲が広がるときは、休止していた枠へ現在の全設定を配送する。
        let expanded = values[8] > previous[8];
        let oscillator_changed = expanded || changes.any(0, 8) || changes.contains(12);
        let envelope_changed = expanded || changes.any(14, 20);
        if changes.contains(8) {
            let count = parameters::to_plain(8, values[8]) as usize;
            for voice in &mut self.state.voices[count..self.state.polyphony.max(count)] {
                voice.stop();
            }
            self.state.unused_pending |= count > self.state.polyphony;
            self.state.polyphony = count;
            self.state.used_voices = self.state.used_voices.min(count);
            self.state.next_oscillator_id %= count;
        }
        if changes.contains(8) || changes.contains(parameters::PORTAMENTO) {
            let time = if self.state.polyphony == 1 {
                parameters::to_plain(parameters::PORTAMENTO, values[parameters::PORTAMENTO])
            } else {
                0.0
            };
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_portamento(time);
            }
        }
        if expanded || changes.contains(parameters::PITCH) {
            let cents = parameters::to_plain(parameters::PITCH, values[parameters::PITCH]);
            let ratio = ((self.state.pitch_bend * 2.0 - 1.0) * cents / 1200.0).exp2();
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_pitch_ratio(ratio);
            }
        }
        if expanded || changes.any(130, 132) {
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_velocity_sensitivity([values[130], values[131]]);
            }
        }
        // 変更のないグループの時間換算（pow）なども省く。
        if oscillator_changed || envelope_changed {
            // Voice 0は常に有効で、適用済みの実単位を保持している。
            let mut envelopes = self.state.voices[0].envelope_parameters();
            let cutoff_shape_changed =
                expanded || changes.any(14, 19) || envelopes[1] == DEFAULT_ENVELOPE;
            for (update, first, envelope) in [oscillator_changed, envelope_changed]
                .into_iter()
                .zip([1, 14])
                .zip(&mut envelopes)
                .map(|((update, first), envelope)| (update, first, envelope))
            {
                if update {
                    // 初期実値と正規化値の往復で丸めが変わるため、初回は従来と同じ全換算。
                    if *envelope == DEFAULT_ENVELOPE {
                        *envelope = parameters::envelope(values, first);
                        continue;
                    }
                    for (i, field) in [
                        &mut envelope.attack_ms,
                        &mut envelope.hold_ms,
                        &mut envelope.decay_ms,
                        &mut envelope.sustain,
                        &mut envelope.release_ms,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if changes.contains(first + i) {
                            *field = parameters::to_plain(first + i, values[first + i]);
                        }
                    }
                }
            }
            if oscillator_changed {
                let waveform = parameters::from_normalized(values[0]).expect("validated values");
                let envelope = envelopes[0];
                let unison_voices = parameters::to_plain(6, values[6]) as u32;
                let detune_cents = parameters::to_plain(7, values[7]);
                let phase_mode =
                    parameters::PHASE_MODES[parameters::to_plain(12, values[12]) as usize];
                for voice in &mut self.state.voices[..self.state.polyphony] {
                    voice.set_parameters(
                        waveform,
                        envelope,
                        unison_voices,
                        detune_cents,
                        phase_mode,
                    );
                }
            }
            if envelope_changed {
                let cutoff_envelope = envelopes[1];
                let cutoff_amount = parameters::to_plain(19, values[19]);
                for voice in &mut self.state.voices[..self.state.polyphony] {
                    if cutoff_shape_changed {
                        voice.set_cutoff_envelope(cutoff_envelope, cutoff_amount);
                    } else {
                        voice.set_cutoff_amount(cutoff_amount);
                    }
                }
            }
        }
        if changes.contains(21) {
            self.state.mono_gains =
                panning_gains(1, 0.0, parameters::to_plain(21, values[21]) / 100.0)[0];
        }
        for (osc, volume) in parameters::OSCILLATOR_VOLUMES.into_iter().enumerate() {
            let wave = [0, 23, 26][osc];
            let tune = [None, Some(24), Some(27)][osc];
            if expanded
                || changes.contains(volume)
                || changes.contains(wave)
                || tune.is_some_and(|i| changes.contains(i))
            {
                let waveform =
                    parameters::from_normalized(values[wave]).expect("validated waveform");
                let ratio = tune.map_or(1.0, |i| {
                    2.0_f64.powf(parameters::to_plain(i, values[i]) / 1200.0)
                });
                let gain = 10.0_f64.powf(parameters::to_plain(volume, values[volume]) / 20.0);
                for voice in &mut self.state.voices[..self.state.polyphony] {
                    voice.set_oscillator(osc, waveform, ratio, gain);
                }
            }
        }
        if expanded || changes.any(29, 31) {
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_phase_modulation([values[29] * 2.0, values[30] * 2.0]);
            }
        }
        if changes.any(20, 22) || changes.contains(6) || (values[7] == 0.0) != (previous[7] == 0.0)
        {
            let count = if values[7] == 0.0 {
                1
            } else {
                parameters::to_plain(6, values[6]) as usize
            };
            let old_count = if previous[7] == 0.0 {
                1
            } else {
                parameters::to_plain(6, previous[6]) as usize
            };
            if count != old_count || changes.contains(21) || (count > 1 && changes.contains(20)) {
                self.state.panning_gains = panning_gains(
                    count,
                    parameters::to_plain(20, values[20]) / 100.0,
                    parameters::to_plain(21, values[21]) / 100.0,
                );
            }
        }
        if expanded || changes.any(9, 11) {
            let filter_type = parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize];
            let cutoff_cents = parameters::to_plain(10, values[10]);
            let resonance_q = parameters::to_plain(11, values[11]);
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_filter(filter_type, cutoff_cents, resonance_q);
            }
        } else if changes.contains(11) {
            let resonance_q = parameters::to_plain(11, values[11]);
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_resonance(resonance_q);
            }
        }
        self.save_parameters(values, &changes);
        true
    }

    /// 数値が変わった項目だけを保存する。±0の符号だけなら既存のDSP値を保つ。
    /// VSTの保存値・表示値はPluginが別に保持する。
    #[cfg_attr(windows, inline(never))]
    #[cfg_attr(not(windows), inline)]
    fn save_parameters(
        &mut self,
        values: &[f64; parameters::COUNT],
        changes: &parameters::ParameterChanges,
    ) {
        for (word, mut pending) in changes.words().into_iter().enumerate() {
            while pending != 0 {
                let index = word * 64 + pending.trailing_zeros() as usize;
                self.state.parameters[index] = values[index];
                pending &= pending - 1;
            }
        }
    }

    pub fn note_on(&mut self, note: Note, tuning: f32, velocity: f32) -> bool {
        // 空き枠探索はせず、指定された循環順で発音中の枠も再使用する。
        if !self.state.voices[self.state.next_oscillator_id].note_on(note, tuning, velocity) {
            return false;
        }
        self.state.used_voices = self
            .state
            .used_voices
            .max(self.state.next_oscillator_id + 1);
        self.state.next_oscillator_id = (self.state.next_oscillator_id + 1) % self.state.polyphony;
        true
    }

    pub fn note_off(&mut self, note: Note) {
        // 同じID、またはID未提供の同チャンネル・同音高が複数あれば全一致をreleaseする。
        for voice in &mut self.state.voices[..self.state.used_voices] {
            voice.note_off(note);
        }
    }

    pub fn stop(&mut self) {
        self.state.effects.reset();
        self.state.eq.reset();
        self.state.limiter.reset();
        self.state.used_voices = 0;
        self.state.unused_pending = true;
        for voice in &mut self.state.voices {
            voice.stop();
        }
    }

    /// 設定・サンプルレート・テンポを保ち、発音と全DSP履歴を初期化する。
    pub fn reset(&mut self) {
        self.stop();
        self.state.next_oscillator_id = 0;
        for voice in &mut self.state.voices {
            voice.reset_oscillators();
        }
    }

    /// 表示用の同時発音数。ブロック末尾で読み、ユニゾンや発振器数は加算しない。
    pub fn active_voice_count(&self) -> u32 {
        self.state.voices[..self.state.used_voices]
            .iter()
            .filter(|voice| voice.is_active())
            .count() as u32
    }

    /// パラメーターとノートが一定の区間を、Voiceごとのサンプル系列で加算する。
    pub fn generate_samples(&mut self, output: &mut [[f64; 2]]) {
        if output.is_empty() {
            return;
        }
        if self.state.unused_pending {
            self.update_unused_sample();
        }
        #[cfg(windows)]
        if self.state.used_voices > 1
            && parameters::plain_for::<9>(self.state.parameters[9]) != 0.0
            && parameters::plain_for::<19>(self.state.parameters[19]) != 0.0
        {
            self.mix_cached_samples(output);
            self.state.effects.process_samples(output);
            self.finish_samples(output);
            return;
        }
        if let Some((first, rest)) = self.state.voices[..self.state.used_voices].split_first_mut() {
            // 先頭Voiceは直接出力し、ゼロ埋め・最初の加算・小区間ごとの再準備を省く。
            first.generate_samples(
                self.state.sample_rate,
                &self.state.panning_gains,
                &self.state.mono_gains,
                output,
            );
            for voice in rest {
                let mut voice_output = [[0.0; 2]; 32];
                for chunk in output.chunks_mut(32) {
                    voice.generate_samples(
                        self.state.sample_rate,
                        &self.state.panning_gains,
                        &self.state.mono_gains,
                        &mut voice_output[..chunk.len()],
                    );
                    for (out, value) in chunk.iter_mut().zip(&voice_output) {
                        out[0] += value[0];
                        out[1] += value[1];
                    }
                }
            }
        } else {
            output.fill([0.0; 2]);
        }
        self.state.effects.process_samples(output);
        self.finish_samples(output);
    }

    /// 32サンプルごとにVoiceを巡回し、同時刻のカットオフ倍率を共用する。
    #[cfg(windows)]
    fn mix_cached_samples(&mut self, output: &mut [[f64; 2]]) {
        let Some((first, rest)) = self.state.voices[..self.state.used_voices].split_first_mut()
        else {
            output.fill([0.0; 2]);
            return;
        };
        let mut voice_output = [[0.0; 2]; 32];
        for chunk in output.chunks_mut(32) {
            let mut caches = std::array::from_fn(|_| CutoffRatioCache::new());
            first.generate_cached_samples(
                self.state.sample_rate,
                &self.state.panning_gains,
                &self.state.mono_gains,
                chunk,
                &mut caches,
            );
            for voice in &mut *rest {
                voice.generate_cached_samples(
                    self.state.sample_rate,
                    &self.state.panning_gains,
                    &self.state.mono_gains,
                    &mut voice_output[..chunk.len()],
                    &mut caches,
                );
                for (out, value) in chunk.iter_mut().zip(&voice_output) {
                    out[0] += value[0];
                    out[1] += value[1];
                }
            }
        }
    }

    pub fn generate_sample(&mut self) -> [f64; 2] {
        if self.state.unused_pending {
            self.update_unused_sample();
        }
        let sum = self.mix_sample();
        let processed = self.state.effects.process_sample(sum);
        let equalized = self.state.eq.process_sample(processed);
        self.state
            .limiter
            .process_sample(equalized)
            .map(|value| value * self.state.master_gain)
    }

    /// EQ・Limiter・Masterを系列で適用する。
    #[inline(never)]
    fn finish_samples(&mut self, samples: &mut [[f64; 2]]) {
        if self.state.eq.enabled() {
            for sample in &mut *samples {
                *sample = self.state.eq.process_sample(*sample);
            }
        }
        self.state.limiter.process_samples(samples);
        for sample in samples {
            *sample = sample.map(|value| value * self.state.master_gain);
        }
    }

    /// 未使用枠の遅延停止を消化し、後続ノートと同時刻の停止として残さない。
    #[cold]
    #[inline(never)]
    fn update_unused_sample(&mut self) {
        if !(self.state.sample_rate.is_finite() && self.state.sample_rate > 0.0) {
            return;
        }
        for voice in &mut self.state.voices[self.state.used_voices..self.state.polyphony] {
            voice.generate_sample(
                self.state.sample_rate,
                &self.state.panning_gains,
                &self.state.mono_gains,
            );
        }
        self.state.unused_pending = false;
    }

    /// フィルタまでのVoice出力を加算する。
    fn mix_sample(&mut self) -> [f64; 2] {
        // 単一Voiceには再利用先がないため、共有cacheの照合自体を省く。
        #[cfg(windows)]
        if self.state.used_voices > 1 {
            return self.mix_cached_sample();
        }
        let mut sum = [0.0; 2];
        for voice in &mut self.state.voices[..self.state.used_voices] {
            let sample = voice.generate_sample(
                self.state.sample_rate,
                &self.state.panning_gains,
                &self.state.mono_gains,
            );
            sum[0] += sample[0];
            sum[1] += sample[1];
        }
        sum
    }

    #[cfg(windows)]
    #[inline(never)]
    fn mix_cached_sample(&mut self) -> [f64; 2] {
        // Windows CRTの指数計算を、同じサンプル時刻のVoice間で共用する。
        let mut cache = CutoffRatioCache::new();
        let mut sum = [0.0; 2];
        for voice in &mut self.state.voices[..self.state.used_voices] {
            let sample = voice.generate_cached_sample(
                self.state.sample_rate,
                &self.state.panning_gains,
                &self.state.mono_gains,
                &mut cache,
            );
            sum[0] += sample[0];
            sum[1] += sample[1];
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn automation_candidates_match_full_parameter_updates() {
        let mut actual = Synthesizer::new();
        assert!(actual.set_sample_rate(48000.0));
        let mut values = parameters::sine_preset();
        actual.set_parameters(&values);
        let mut reference = actual.clone();
        for first in 0..parameters::COUNT {
            let mut candidates = [0; parameters::COUNT.div_ceil(64)];
            for index in [
                first,
                (first + 63) % parameters::COUNT,
                (first + 127) % parameters::COUNT,
            ] {
                values[index] = (values[index] + 0.25) % 1.0;
                candidates[index / 64] |= 1 << (index % 64);
            }
            assert_eq!(
                actual.set_automated_parameters(&values, candidates),
                reference.set_parameters(&values)
            );
            assert!(!actual.set_automated_parameters(&values, candidates));
            if first % 17 == 0 {
                let note = Note {
                    id: first as i32,
                    channel: 0,
                    pitch: 60 + (first % 12) as i16,
                };
                actual.note_on(note, 0.0, 0.8);
                reference.note_on(note, 0.0, 0.8);
            }
            assert_eq!(actual.parameters(), reference.parameters());
            for _ in 0..8 {
                assert_eq!(actual.generate_sample(), reference.generate_sample());
            }
        }
    }

    #[test]
    fn numeric_snapshot_preserves_audio_with_mixed_signed_zero_changes() {
        let mut actual = super::Synthesizer::new();
        assert!(actual.set_sample_rate(44100.0));
        let mut values = crate::parameters::sine_preset();
        actual.set_parameters(&values);
        actual.note_on(
            crate::voice::Note {
                id: 1,
                channel: 0,
                pitch: 69,
            },
            0.0,
            1.0,
        );
        let mut reference = actual.clone();
        for step in 0..8 {
            for value in &mut values {
                if *value == 0.0 {
                    *value = if step % 2 == 0 { -0.0 } else { 0.0 };
                }
            }
            values[13] = crate::parameters::to_normalized(13, [-12.0, -6.0, 0.0][step % 3]);
            actual.set_parameters(&values);
            reference.set_parameters(&values);
            reference.state.parameters = values;
            assert_eq!(actual.parameters(), values);
            let mut a = [[0.0; 2]; 257];
            let mut b = a;
            actual.generate_samples(&mut a);
            reference.generate_samples(&mut b);
            assert_eq!(a, b);
        }
    }

    use super::*;
    use crate::envelope::DEFAULT_ENVELOPE;
    use crate::oscillator::Waveform;

    // 既存の発振器・ノート処理の期待値はフィルタをバイパスして検証する。
    fn unfiltered() -> Synthesizer {
        let mut synth = Synthesizer::new();
        let mut values = parameters::defaults();
        values[9] = 0.0;
        synth.set_parameters(&values);
        synth
    }

    fn note(id: i32, channel: i16, pitch: i16) -> Note {
        Note { id, channel, pitch }
    }

    #[test]
    fn note_controls_apply_bend_range_and_restrict_glide_to_mono() {
        let mut synth = Synthesizer::new();
        let mut values = parameters::sine_preset();
        values[parameters::PORTAMENTO] = parameters::to_normalized(parameters::PORTAMENTO, 10.0);
        values[parameters::PITCH] = 1.0;
        synth.set_parameters(&values);
        synth.note_on(
            Note {
                id: 1,
                channel: 0,
                pitch: 69,
            },
            0.0,
            1.0,
        );
        synth.generate_sample();
        assert_eq!(
            synth.state.voices[0].state.oscillator_inputs[0].frequency_hz,
            440.0
        );
        for (bend, hz) in [(1.0, 880.0), (0.0, 220.0), (0.5, 440.0)] {
            synth.set_pitch_bend(bend);
            for input in synth.state.voices[0].state.oscillator_inputs {
                assert_eq!(input.frequency_hz, hz);
            }
        }
        synth.note_on(
            Note {
                id: 2,
                channel: 0,
                pitch: 81,
            },
            0.0,
            1.0,
        );
        assert_eq!(
            synth.state.voices[0].state.oscillator_inputs[0].frequency_hz,
            440.0
        );
        values[8] = parameters::to_normalized(8, 2.0);
        synth.set_parameters(&values); // Poly変更は進行中の移行も終了する。
        assert_eq!(
            synth.state.voices[0].state.oscillator_inputs[0].frequency_hz,
            880.0
        );
        synth.note_on(
            Note {
                id: 3,
                channel: 0,
                pitch: 57,
            },
            0.0,
            1.0,
        );
        assert_eq!(
            synth.state.voices[0].state.oscillator_inputs[0].frequency_hz,
            220.0
        );
        synth.set_pitch_bend(1.0);
        values[parameters::PITCH] = 0.0;
        synth.set_parameters(&values); // 幅0では受信値によらず元の音高。
        assert_eq!(
            synth.state.voices[0].state.oscillator_inputs[0].frequency_hz,
            220.0
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 全エフェクトの履歴を充填する音声試験は実CPUで行う。
    fn reset_clears_history_and_replays_from_initial_state_with_current_settings() {
        for kind in 0..7 {
            for phase in 0..3 {
                let mut synth = Synthesizer::new();
                let rate = [44100.0, 48000.0, 96000.0][phase];
                assert!(synth.set_sample_rate(rate));
                let mut values = parameters::sine_preset();
                for (index, value) in [
                    (0, (kind % 5) as f64),
                    (6, 7.0),
                    (7, 25.0),
                    (8, 4.0),
                    (9, 2.0),
                    (10, 1200.0),
                    (31, 1.0),
                    (32, 12.0),
                    (12, phase as f64),
                    (20, 30.0),
                    (25, -12.0),
                    (29, 80.0),
                    (30, 60.0),
                    (34, 1.0),
                    (36, 6.0),
                    (38, -6.0),
                    (108, 1.0),
                ] {
                    values[index] = parameters::to_normalized(index, value);
                }
                for indices in parameters::EFFECT_SLOTS {
                    values[indices[0]] = parameters::to_normalized(indices[0], kind as f64);
                    values[indices[4]] = 0.0; // 最短音価で再発音後のテンポ依存も照合する。
                }
                synth.set_parameters(&values);
                synth.set_tempo(173.0);
                let mut fresh = synth.clone();
                for id in 0..3 {
                    assert!(synth.note_on(note(id, 0, 60 + id as i16 * 7), 0.0, 0.8));
                }
                let mut buffer = [[0.0; 2]; 64];
                for _ in 0..256 {
                    synth.generate_samples(&mut buffer);
                }
                synth.reset();
                synth.reset(); // 連打しても初期状態を保つ。
                assert_eq!(synth.parameters(), values);
                assert_eq!(synth.state.next_oscillator_id, 0);
                assert_eq!(synth.active_voice_count(), 0);
                assert_eq!(synth.latency_samples(), fresh.latency_samples());
                assert_eq!(synth.state.sample_rate, rate);
                // Keepではリセット直後、Zero/Randomでは無音区間を挟んで再発音する。
                for _ in 0..if phase == 0 { 0 } else { 256 } {
                    synth.generate_samples(&mut buffer);
                    assert_eq!(buffer, [[0.0; 2]; 64], "kind={kind}, phase={phase}");
                    fresh.generate_samples(&mut buffer);
                }
                for id in 0..4 {
                    for instrument in [&mut synth, &mut fresh] {
                        assert!(instrument.note_on(note(id, 0, 65 + id as i16), 0.0, 0.7));
                    }
                }
                let mut expected = [[0.0; 2]; 64];
                for _ in 0..64 {
                    synth.generate_samples(&mut buffer);
                    fresh.generate_samples(&mut expected);
                    for (a, b) in buffer.iter().flatten().zip(expected.iter().flatten()) {
                        assert!(
                            (a - b).abs() < 1e-4,
                            "kind={kind}, phase={phase}: {a} != {b}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unused_stop_is_consumed_only_after_a_valid_sample() {
        for resized in [false, true] {
            for advance in 0..6 {
                let mut synth = Synthesizer::new();
                let mut values = parameters::sine_preset();
                values[5] = parameters::to_normalized(5, 100.0);
                values[8] = parameters::to_normalized(8, if resized { 2.0 } else { 1.0 });
                synth.set_parameters(&values);
                synth.set_sample_rate(48000.0);
                if resized {
                    // 2枠目を停止して外し、再拡大後も未使用のままにする。
                    values[8] = parameters::to_normalized(8, 1.0);
                    synth.set_parameters(&values);
                    synth.generate_samples(&mut [[0.0; 2]; 8]);
                    values[8] = parameters::to_normalized(8, 2.0);
                    synth.set_parameters(&values);
                    assert!(synth.note_on(note(1, 0, 60), 0.0, 0.0));
                } else {
                    synth.stop();
                }
                match advance {
                    0 => {}
                    1 => synth.generate_samples(&mut []),
                    2 => {
                        synth.generate_sample();
                    }
                    3 => synth.generate_samples(&mut [[0.0; 2]; 64]),
                    4 | 5 => {
                        synth.set_sample_rate(f64::NAN);
                        synth.generate_samples(&mut [[0.0; 2]; 64]);
                        assert!(synth.state.unused_pending);
                        synth.set_sample_rate(48000.0);
                        if advance == 5 {
                            synth.generate_sample();
                        }
                    }
                    _ => unreachable!(),
                }
                let next = note(2, 0, 69);
                assert!(synth.note_on(next, 0.0, 1.0));
                synth.note_off(next);
                let mut output = vec![[0.0; 2]; synth.latency_samples() + 64];
                synth.generate_samples(&mut output);
                let peak = output.iter().flatten().fold(0.0_f64, |p, v| p.max(v.abs()));
                let advanced = matches!(advance, 2 | 3 | 5);
                assert_eq!(peak > 0.1, advanced, "resized={resized}, advance={advance}");
                assert_eq!(
                    synth.active_voice_count(),
                    u32::from(resized) + u32::from(advanced)
                );
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 多数の音声境界のビット比較は実CPUで行う。
    fn constant_input_series_matches_sample_processing_across_boundaries() {
        for phase in 0..3 {
            let mut values = parameters::sine_preset();
            for (index, value) in [(5, 1.), (12, phase as f64), (32, 18.), (33, 65.), (42, 1.)] {
                values[index] = parameters::to_normalized(index, value);
            }
            let mut serial = Synthesizer::new();
            serial.set_sample_rate(48000.);
            serial.set_parameters(&values);
            // ノイズ・Random位相を含む全ての初期乱数状態を同じにする。
            let mut series = serial.clone();
            let mut heard = false;
            for step in 0..50 {
                let changes: &[(usize, f64)] = match step {
                    1 => &[(24, 600.)],
                    2 => &[(6, 7.)],
                    3 => &[(7, 8.)],
                    5 => &[(0, 1.)],
                    6 => &[(27, -700.)],
                    7 => &[(12, 2.)],
                    11 => &[(29, 0.)],
                    13 => &[(29, 37.)],
                    15 => &[(24, 0.)],
                    19 => &[(7, 0.)],
                    22 => &[(4, 0.43)],
                    4 => &[
                        (0, 3.),
                        (6, 3.),
                        (7, 25.),
                        (8, 4.),
                        (9, 2.),
                        (10, 1200.),
                        (11, 2.),
                        (14, 3.),
                        (15, 1.),
                        (16, 5.),
                        (17, 0.25),
                        (18, 7.),
                        (19, 1200.),
                        (20, 75.),
                        (23, 1.),
                        (25, -12.),
                        (26, 2.),
                        (28, -12.),
                        (29, 23.),
                        (30, 17.),
                    ],
                    8 => &[(31, 1.)],
                    12 => &[(0, 4.), (23, 4.), (26, 0.)],
                    16 => &[(31, 2.)],
                    20 => &[(0, 0.), (23, 2.), (26, 1.), (25, -60.), (28, -60.)],
                    24 => &[(31, 0.), (9, 0.)],
                    28 => &[(29, 0.), (30, 0.), (9, 2.)],
                    32 => &[(25, -12.), (28, -12.), (29, 23.), (30, 17.)],
                    36 => &[(23, 4.), (29, 100.)],
                    40 => &[(5, 0.), (9, 0.), (18, 0.)],
                    44 => &[(29, 0.), (30, 0.)],
                    _ => &[],
                };
                for &(index, value) in changes {
                    values[index] = parameters::to_normalized(index, value);
                }
                let events: &[(bool, i32, i16)] = match step {
                    0 => &[(true, 1, 60)],
                    4 => &[(true, 2, 64), (true, 3, 67)],
                    10 => &[(false, 2, 64), (true, 4, 64)],
                    16 => &[(true, 5, 64), (false, 4, 64)],
                    27 => &[(true, 6, 60)],
                    31 => &[(false, 6, 60), (true, 7, 60)],
                    35 => &[(true, 8, 60), (false, 7, 60)],
                    41 => &[(true, 9, 60)],
                    42 => &[(false, 9, 60)],
                    45 => &[(true, 10, 60)],
                    46 => &[(false, 10, 60)],
                    48 => &[(true, 11, 60)],
                    _ => &[],
                };
                for synth in [&mut serial, &mut series] {
                    if step == 9 {
                        synth.set_sample_rate(44100.);
                    }
                    if step == 17 {
                        synth.set_sample_rate(96000.);
                    }
                    synth.set_parameters(&values);
                    for &(on, id, pitch) in events {
                        if on {
                            assert!(synth.note_on(note(id, 0, pitch), 0., 0.8));
                        } else {
                            synth.note_off(note(id, 0, pitch));
                        }
                    }
                    if step == 21 {
                        for id in 1..=5 {
                            synth.note_off(note(id, 0, 64));
                        }
                    }
                    if matches!(step, 24 | 39) {
                        synth.stop();
                    }
                }
                let count = [0, 1, 2, 7, 31, 32, 33, 127][step % 8];
                let mut output = [[f64::NAN; 2]; 127];
                series.generate_samples(&mut output[..count]);
                for value in &output[..count] {
                    let expected = serial.generate_sample();
                    assert_eq!(
                        value.map(f64::to_bits),
                        expected.map(f64::to_bits),
                        "phase={phase}, step={step}"
                    );
                    heard |= *value != [0.; 2];
                }
                assert_eq!(series.active_voice_count(), serial.active_voice_count());
                if matches!(step, 43 | 44 | 47) {
                    assert!(
                        output[series.latency_samples().min(count)..count]
                            .iter()
                            .all(|sample| *sample == [0.; 2])
                    );
                    assert_eq!(series.active_voice_count(), 0);
                }
            }
            assert!(heard);
        }
    }

    #[test]
    fn reused_envelopes_preserve_initial_conversion_and_expansion() {
        let mut synth = Synthesizer::new();
        let mut values = parameters::defaults();
        let mut expected = [DEFAULT_ENVELOPE; 2];
        for (index, plain) in [
            (13, 6.),
            (7, 10.),
            (19, 1200.),
            (6, 3.),
            (4, 0.5),
            (16, 500.),
            (8, 32.),
        ] {
            values[index] = parameters::to_normalized(index, plain);
            if index <= 8 {
                expected[0] = parameters::envelope(&values, 1);
            }
            if (14..20).contains(&index) || index == 8 {
                expected[1] = parameters::envelope(&values, 14);
            }
            synth.set_parameters(&values);
            for voice in &synth.state.voices[..synth.state.polyphony] {
                assert_eq!(voice.envelope_parameters(), expected);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 設定全項目の音声照合は実CPUで行う。
    fn increasing_polyphony_synchronizes_settings_changed_while_disabled() {
        let mut delayed = Synthesizer::new();
        let mut reference = Synthesizer::new();
        let mut values = parameters::sine_preset();
        values[8] = parameters::to_normalized(8, 4.0);
        for synth in [&mut delayed, &mut reference] {
            synth.set_sample_rate(48000.0);
            synth.set_parameters(&values);
            for id in 0..4 {
                assert!(synth.note_on(note(id, 0, 60 + id as i16), 0.0, 0.8));
            }
            for _ in 0..128 {
                synth.generate_sample();
            }
            let mut limited = values;
            limited[8] = 0.0;
            synth.set_parameters(&limited);
        }
        reference.set_parameters(&values);
        for (i, plain) in [
            (0, 3.),
            (1, 2.),
            (2, 1.),
            (3, 7.),
            (4, 0.4),
            (5, 3.),
            (6, 3.),
            (7, 17.),
            (9, 6.),
            (10, 1200.),
            (11, 2.),
            (12, 0.),
            (14, 1.),
            (15, 2.),
            (16, 5.),
            (17, 0.3),
            (18, 4.),
            (19, -2400.),
            (22, -6.),
            (23, 1.),
            (24, 300.),
            (25, -12.),
            (26, 2.),
            (27, -700.),
            (28, -9.),
            (29, 33.),
            (30, 71.),
            (31, 1.),
            (32, 18.),
            (33, 75.),
            (130, 25.),
            (131, 75.),
        ] {
            values[i] = parameters::to_normalized(i, plain);
        }
        reference.set_parameters(&values);
        let mut limited = values;
        limited[8] = 0.0;
        delayed.set_parameters(&limited);
        delayed.set_parameters(&values);
        for id in 0..4 {
            let n = note(id + 20, 0, 48 + id as i16 * 3);
            assert!(delayed.note_on(n, 0.0, 0.7));
            assert!(reference.note_on(n, 0.0, 0.7));
        }
        for _ in 0..4096 {
            assert_eq!(delayed.generate_sample(), reference.generate_sample());
        }
    }

    #[test]
    fn voice_count_tracks_allocation_release_limits_and_stop() {
        let mut synth = unfiltered();
        synth.set_sample_rate(48000.0);
        let mut values = parameters::sine_preset();
        values[5] = parameters::to_normalized(5, 1.0); // 48サンプルのrelease。
        values[8] = parameters::to_normalized(8, 32.0);
        values[6] = 1.0; // ユニゾン7でもVoiceの数は変わらない。
        values[7] = 1.0;
        synth.set_parameters(&values);
        assert_eq!(synth.active_voice_count(), 0);
        for id in 0..33 {
            assert!(synth.note_on(note(id, 0, 60), 0.0, 1.0));
            synth.mix_sample();
            assert_eq!(synth.active_voice_count(), (id + 1).min(32) as u32);
        }
        // 上書きされた旧ノートのOffは新しいノートを消さない。
        synth.note_off(note(0, 0, 60));
        assert_eq!(synth.active_voice_count(), 32);
        for id in 1..33 {
            synth.note_off(note(id, 0, 60));
        }
        for _ in 0..48 {
            synth.mix_sample();
            assert_eq!(synth.active_voice_count(), 32);
        }
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 0);
        for id in 0..32 {
            synth.note_on(note(id, 0, 60), 0.0, 1.0);
        }
        values[8] = parameters::to_normalized(8, 2.0);
        synth.set_parameters(&values);
        assert_eq!(synth.active_voice_count(), 2);
        synth.stop();
        assert_eq!(synth.active_voice_count(), 0);
        values[8] = 1.0;
        synth.set_parameters(&values);
        assert_eq!(synth.active_voice_count(), 0);
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 0);
    }

    #[test]
    fn maximum_phase_modulation_stays_finite_with_high_notes_and_all_waveforms() {
        // Miriは最小レートで全波形・最大枠の系列を通し、レート別の数値走査は実CPUで行う。
        let rates: &[f64] = if cfg!(miri) {
            &[44100.0]
        } else {
            &[44100.0, 48000.0, 96000.0]
        };
        for &rate in rates {
            for wave in 0..5 {
                let mut synth = Synthesizer::new();
                synth.set_sample_rate(rate);
                let mut values = parameters::sine_preset();
                for index in [0, 23, 26] {
                    values[index] = parameters::to_normalized(index, f64::from(wave));
                }
                for index in [6, 7, 8, 11, 24, 27, 29, 30] {
                    values[index] = 1.0;
                }
                for index in [13, 22, 25, 28] {
                    values[index] = parameters::to_normalized(index, 12.0);
                }
                values[9] = parameters::to_normalized(9, 2.0);
                synth.set_parameters(&values);
                for id in 0..32 {
                    assert!(synth.note_on(note(id, 0, 127), 0.0, 1.0));
                }
                if cfg!(miri) {
                    // 全設定で準備済みPM系列と、ADAA履歴充填後の更新を通す。
                    let mut output = [[f64::NAN; 2]; 8];
                    synth.generate_samples(&mut output);
                    assert!(output.iter().flatten().all(|x| x.is_finite()));
                } else {
                    for _ in 0..33 {
                        assert!(synth.mix_sample().iter().all(|x| x.is_finite()));
                    }
                }
                synth.stop();
                assert_eq!(synth.mix_sample(), [0.0; 2]);
            }
        }
    }

    #[test]
    fn shared_panning_follows_unison_detune_and_inactive_settings() {
        for polyphony in [1, 4] {
            let mut synth = polyphonic(polyphony);
            let mut values = synth.state.parameters;
            let mut phases = [0.0_f64; 7];
            for id in 0..polyphony {
                synth.note_on(note(id as i32, 0, 0), 0.0, 1.0);
            }
            for unison in 1..=7 {
                for detune in [25.0, 0.0, 25.0] {
                    for width in [0.0, 0.73, 1.0] {
                        for pan in [-1.0, -0.37, 0.0, 1.0] {
                            values[6] = parameters::to_normalized(6, unison as f64);
                            values[7] = parameters::to_normalized(7, detune);
                            values[20] = parameters::to_normalized(20, width * 100.0);
                            values[21] = parameters::to_normalized(21, pan * 100.0);
                            synth.set_parameters(&values);
                            let count = if detune == 0.0 { 1 } else { unison };
                            let mut expected = [0.0; 2];
                            for (k, phase) in phases.iter_mut().enumerate().take(count) {
                                let spread = if count == 1 {
                                    0.0
                                } else {
                                    2.0 * k as f64 / (count - 1) as f64 - 1.0
                                };
                                let position = pan + (1.0 - pan.abs()) * width * spread;
                                let angle = (position + 1.0) * std::f64::consts::FRAC_PI_4;
                                let gain = (std::f64::consts::TAU * *phase).sin()
                                    * polyphony as f64
                                    / (count as f64).sqrt();
                                let hz = 440.0
                                    * 2.0_f64.powf(-69.0 / 12.0)
                                    * 2.0_f64.powf(detune * spread / 1200.0);
                                *phase = (*phase + hz / 48000.0).fract();
                                expected[0] += gain * angle.cos();
                                expected[1] += gain * angle.sin();
                            }
                            let actual = synth.mix_sample();
                            for ch in 0..2 {
                                assert!(
                                    (actual[ch] - expected[ch]).abs() < polyphony as f64 * 1e-6
                                );
                            }
                        }
                    }
                }
            }
            synth.stop();
            values[12] = parameters::to_normalized(12, 1.0); // 再発音時の位相を揃える。
            values[20] = 0.43;
            values[21] = 0.31;
            synth.set_parameters(&values);
            assert_eq!(synth.mix_sample(), [0.0; 2]);
            // 保存復元と同じ、発音前の設定配送が新しい全枠へ適用される。
            let mut restored = Synthesizer::new();
            restored.set_sample_rate(48000.0);
            restored.set_parameters(&values);
            for id in 0..polyphony {
                synth.note_on(note(id as i32, 0, 0), 0.0, 1.0);
                restored.note_on(note(id as i32, 0, 0), 0.0, 1.0);
            }
            assert_eq!(synth.mix_sample(), restored.mix_sample());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 全パラメーター・端点・枠数の音声照合は実CPUで行う。
    fn individual_parameter_updates_reach_current_and_future_voices() {
        for index in (0..34).chain([130, 131]).filter(|&i| i != 8 && i != 12) {
            for value in [0.0, 1.0] {
                for count in [1, 32] {
                    let mut synth = Synthesizer::new();
                    synth.set_sample_rate(48000.0);
                    let mut values = parameters::defaults();
                    // この比較は独立Voiceの周期波を照合する。Noiseは別の統計検証で扱う。
                    values[index] = if parameters::is_waveform(index) {
                        value * 0.75
                    } else {
                        value
                    };
                    synth.set_parameters(&values);
                    // 設定を配送した後に使用枠を増やす。
                    values[8] = parameters::to_normalized(8, f64::from(count));
                    synth.set_parameters(&values);
                    let mut reference = Voice::new();
                    reference.set_parameters(
                        parameters::from_normalized(values[0]).unwrap(),
                        parameters::envelope(&values, 1),
                        parameters::to_plain(6, values[6]) as u32,
                        parameters::to_plain(7, values[7]),
                        crate::oscillator::PhaseMode::Keep,
                    );
                    reference.set_filter(
                        parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize],
                        parameters::to_plain(10, values[10]),
                        parameters::to_plain(11, values[11]),
                    );
                    reference.set_cutoff_envelope(
                        parameters::envelope(&values, 14),
                        parameters::to_plain(19, values[19]),
                    );
                    for (osc, volume) in parameters::OSCILLATOR_VOLUMES.into_iter().enumerate() {
                        let wave = [0, 23, 26][osc];
                        let ratio = if osc == 0 {
                            1.0
                        } else {
                            2.0_f64.powf(parameters::to_plain(wave + 1, values[wave + 1]) / 1200.0)
                        };
                        reference.set_oscillator(
                            osc,
                            parameters::from_normalized(values[wave]).unwrap(),
                            ratio,
                            10.0_f64.powf(parameters::to_plain(volume, values[volume]) / 20.0),
                        );
                    }
                    reference.set_phase_modulation([values[29] * 2.0, values[30] * 2.0]);
                    reference.set_velocity_sensitivity([values[130], values[131]]);
                    reference.note_on(note(0, 0, 69), 0.0, 0.25);
                    for id in 0..count {
                        synth.note_on(note(id, 0, 69), 0.0, 0.25);
                    }
                    for _ in 0..96 {
                        let sample = reference.generate_sample(
                            48000.0,
                            &synth.state.panning_gains,
                            &synth.state.mono_gains,
                        );
                        let expected = sample.map(|s| (0..count).map(|_| s).sum::<f64>());
                        // 初期値と対数軸の往復換算値には浮動小数点の丸め差がある。
                        for (actual, expected) in synth.mix_sample().into_iter().zip(expected) {
                            assert!(
                                (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                                "parameter {index}: {actual} != {expected}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn amount_only_updates_preserve_envelope_history_across_rate_and_voice_changes() {
        let mut synth = Synthesizer::new();
        let mut reference = Synthesizer::new();
        let mut values = parameters::defaults();
        values[9] = parameters::to_normalized(9, 2.0);
        values[12] = parameters::to_normalized(12, 1.0);
        for sample in 0..1024 {
            if sample % 64 == 0 {
                values[19] = parameters::to_normalized(19, (sample % 257) as f64 * 16.0);
                if sample == 128 {
                    values[14] = parameters::to_normalized(14, 20.0);
                }
                if sample == 256 {
                    values[8] = parameters::to_normalized(8, 4.0);
                }
                if sample == 512 {
                    values[14..19].copy_from_slice(&parameters::defaults()[14..19]);
                }
                if sample == 768 {
                    values[8] = parameters::to_normalized(8, 1.0);
                }
                synth.set_parameters(&values);
                reference.set_parameters(&values);
                // Amountの更新でも従来どおりAHDSR全体を通知する基準経路。
                for voice in &mut reference.state.voices[..reference.state.polyphony] {
                    voice.set_cutoff_envelope(
                        parameters::envelope(&values, 14),
                        parameters::to_plain(19, values[19]),
                    );
                }
            }
            if sample == 0 || sample == 256 || sample == 768 {
                for id in 0..synth.state.polyphony {
                    let event = note(id as i32, 0, 60 + id as i16);
                    synth.note_on(event, 0.0, 0.8);
                    reference.note_on(event, 0.0, 0.8);
                }
            }
            if sample == 640 {
                for id in 0..synth.state.polyphony {
                    let event = note(id as i32, 0, 60 + id as i16);
                    synth.note_off(event);
                    reference.note_off(event);
                }
            }
            if sample == 384 {
                assert!(synth.set_sample_rate(96000.0));
                assert!(reference.set_sample_rate(96000.0));
            }
            assert_eq!(
                synth.mix_sample(),
                reference.mix_sample(),
                "sample {sample}"
            );
        }
    }

    fn polyphonic(count: usize) -> Synthesizer {
        let mut synth = unfiltered();
        let mut values = parameters::defaults();
        values[9] = 0.0;
        values[0] = parameters::normalized(Waveform::Sine);
        values[1] = 0.0;
        values[3] = 0.0;
        values[4] = 1.0;
        values[5] = 0.0;
        values[8] = parameters::to_normalized(8, count as f64);
        synth.set_parameters(&values);
        synth.set_sample_rate(48000.0);
        synth
    }

    #[test]
    fn global_effect_matches_single_sample_and_batched_processing() {
        let mut reference = Synthesizer::new();
        reference.set_sample_rate(48000.0);
        let mut values = parameters::sine_preset();
        values[34] = 1.0;
        values[36] = parameters::to_normalized(36, 6.0);
        reference.set_parameters(&values);
        reference.note_on(note(1, 0, 60), 0.0, 1.0);
        let mut batched = reference.clone();
        for block in 0..128 {
            if block == 40 {
                reference.note_off(note(1, 0, 60));
                batched.note_off(note(1, 0, 60));
            }
            let mut output = [[0.0; 2]; 127];
            batched.generate_samples(&mut output);
            for value in output {
                let expected = reference.generate_sample();
                for ch in 0..2 {
                    assert!((value[ch] - expected[ch]).abs() < 1e-10);
                }
            }
        }
        batched.stop();
        assert_eq!(batched.generate_sample(), [0.0; 2]);
    }

    #[test]
    fn master_volume_scales_the_sum_of_filtered_voices_and_their_tails() {
        for filter_type in 0..7 {
            let mut synth = Synthesizer::new();
            let mut values = parameters::defaults();
            values[1] = 0.0;
            values[3] = 0.0;
            values[4] = 1.0;
            values[5] = 0.0;
            values[8] = parameters::to_normalized(8, 2.0);
            values[9] = parameters::to_normalized(9, f64::from(filter_type));
            values[10] = parameters::to_normalized(10, -1200.0);
            values[11] = parameters::to_normalized(11, 3.0);
            values[14] = parameters::to_normalized(14, 0.5);
            values[16] = parameters::to_normalized(16, 1.0);
            values[19] = parameters::to_normalized(19, 1200.0);
            synth.set_parameters(&values);
            let mut references: [Synthesizer; 2] = std::array::from_fn(|_| {
                let mut single = Synthesizer::new();
                let mut values = values;
                values[8] = 0.0;
                single.set_parameters(&values);
                single
            });
            let mut delayed_expected =
                std::collections::VecDeque::from(vec![0.0; synth.latency_samples()]);
            for n in 0..96 + synth.latency_samples() {
                let db = match n.saturating_sub(synth.latency_samples()) {
                    0..16 => 0.0,
                    16..32 => -48.0,
                    32..48 => -12.3,
                    48..72 => 12.0,
                    _ => 0.0,
                };
                values[13] = parameters::to_normalized(13, db);
                synth.set_parameters(&values);
                for (at, slot, id, pitch) in [(0, 0, 1, 57), (3, 1, 2, 81), (32, 0, 3, 69)] {
                    if n == at {
                        synth.note_on(note(id, 0, pitch), 0.0, 1.0);
                        references[slot].note_on(note(id, 0, pitch), 0.0, 1.0);
                    }
                }
                if n == 48 {
                    synth.note_off(note(2, 0, 81));
                    references[1].note_off(note(2, 0, 81));
                }
                if n == 64 {
                    values[8] = 0.0;
                    synth.set_parameters(&values);
                    references[1].stop();
                }
                let expected: f64 = references.iter_mut().map(|s| s.mix_sample()[0]).sum();
                delayed_expected.push_back(expected);
                let expected = delayed_expected.pop_front().unwrap() * 10.0_f64.powf(db / 20.0);
                assert!((synth.generate_sample()[0] - expected).abs() < 1e-12);
            }
            synth.stop();
            assert_eq!(synth.mix_sample()[0], 0.0);
        }
    }

    #[test]
    fn polyphony_cycles_all_counts_and_sums_independent_notes() {
        for count in 1..=32 {
            let mut synth = polyphonic(count);
            for id in 0..count {
                assert_eq!(synth.state.next_oscillator_id, id);
                assert!(synth.note_on(note(id as i32, 0, 60 + id as i16), 0.0, 1.0));
            }
            assert_eq!(synth.state.next_oscillator_id, 0);
            // 全枠の初回サンプルはSineの位相0。
            assert_eq!(synth.mix_sample()[0], 0.0);
            assert!(synth.note_on(note(100, 0, 100), 0.0, 0.5));
            synth.note_off(note(0, 0, 60)); // 上書き済みのノートでは新しい音を消さない。
            assert!(synth.state.voices[0].state.gate);
            let expected = (0..count)
                .map(|i| {
                    let frequency = 440.0 * 2.0_f64.powf((60.0 + i as f64 - 69.0) / 12.0);
                    let velocity = if i == 0 { 0.5 } else { 1.0 };
                    velocity
                        * 0.5
                        * std::f64::consts::SQRT_2
                        * (std::f64::consts::TAU * frequency / 48000.0).sin()
                })
                .sum::<f64>();
            assert!((synth.mix_sample()[0] - expected).abs() < count as f64 * 3e-7);
            let next = synth.state.next_oscillator_id;
            synth.note_off(note(100, 0, 100));
            assert_eq!(synth.state.next_oscillator_id, next);
            for id in 1..count {
                synth.note_off(note(id as i32, 0, 60 + id as i16));
            }
            assert_eq!(synth.mix_sample()[0], 0.0);
            assert!(!synth.note_on(note(200, 16, 60), 0.0, 1.0));
            assert!(!synth.note_on(note(200, 0, 60), f32::MAX, 1.0));
            assert_eq!(synth.state.next_oscillator_id, next);
        }
    }

    #[test]
    fn polyphony_envelopes_and_unison_match_independent_oscillators() {
        let mut synth = polyphonic(3);
        let mut values = synth.state.parameters;
        values[0] = 0.0; // Sineで位相差も比較する。
        values[1] = parameters::to_normalized(1, 5.0);
        values[5] = parameters::to_normalized(5, 10.0);
        values[6] = 1.0;
        values[7] = 0.37;
        synth.set_parameters(&values);
        let mut references: [Synthesizer; 3] = std::array::from_fn(|_| {
            let mut single = polyphonic(1);
            let mut settings = values;
            settings[8] = 0.0;
            single.set_parameters(&settings);
            single
        });
        // 発音時刻・音高・ベロシティが異なる3音。4音目は0番を再使用。
        for sample in 0..96 {
            for (at, slot, id, pitch, velocity) in [
                (0, 0, 1, 60, 1.0),
                (7, 1, 2, 64, 0.5),
                (15, 2, 3, 67, 0.75),
                (48, 0, 4, 72, 0.25),
            ] {
                if sample == at {
                    synth.note_on(note(id, 0, pitch), 0.0, velocity);
                    references[slot].note_on(note(id, 0, pitch), 0.0, velocity);
                }
            }
            if sample == 32 {
                synth.note_off(note(2, 0, 64));
                references[1].note_off(note(2, 0, 64));
            }
            if sample == 64 {
                synth.note_off(note(1, 0, 60));
            } // 0番の新しい音へ影響しない。
            if sample == 80 {
                synth.stop();
                for single in &mut references {
                    single.stop();
                }
            }
            let expected: f64 = references.iter_mut().map(|s| s.mix_sample()[0]).sum();
            assert!((synth.mix_sample()[0] - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn polyphony_resize_stops_removed_notes_and_bounds_next_id() {
        let mut synth = polyphonic(4);
        for id in 0..3 {
            synth.note_on(note(id, 0, 60), 0.0, 1.0);
        }
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 3);
        let mut values = synth.state.parameters;
        values[8] = parameters::to_normalized(8, 2.0);
        synth.set_parameters(&values);
        assert_eq!(synth.state.next_oscillator_id, 1); // 3 % 2
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 2);
        values[8] = parameters::to_normalized(8, 4.0);
        synth.set_parameters(&values);
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 2); // 除外したノートは復活しない。
        synth.note_on(note(4, 0, 60), 0.0, 1.0); // 1番を上書き。
        synth.note_on(note(5, 0, 60), 0.0, 1.0); // 2番を再使用。
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 3);
        // サンプル処理を挟まず減少→増加→再割り当てしても古い振幅を引き継がない。
        values[8] = 0.0;
        synth.set_parameters(&values);
        values[8] = parameters::to_normalized(8, 4.0);
        values[1] = parameters::to_normalized(1, 10.0);
        synth.set_parameters(&values);
        synth.note_on(note(6, 0, 60), 0.0, 1.0);
        synth.note_on(note(7, 0, 60), 0.0, 1.0);
        let phase = 4.0 * 440.0 * 2.0_f64.powf(-9.0 / 12.0) / 48000.0;
        let expected = 0.5 * std::f64::consts::SQRT_2 * (std::f64::consts::TAU * phase).sin();
        assert!((synth.mix_sample()[0] - expected).abs() < 3e-7);
        assert_eq!(synth.active_voice_count(), 2);
        synth.stop();
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 0);
    }

    #[test]
    fn polyphony_note_off_matches_ids_and_missing_id_channel_pitch() {
        let mut synth = polyphonic(4);
        for n in [
            note(1, 0, 60),
            note(2, 0, 60),
            note(-1, 1, 60),
            note(-1, 1, 60),
        ] {
            synth.note_on(n, 0.0, 1.0);
        }
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 4);
        synth.note_off(note(1, 0, 60));
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 3);
        synth.note_off(note(-1, 2, 60)); // 別チャンネルは無関係。
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 3);
        synth.note_off(note(-1, 1, 60)); // IDなし同音の全一致をrelease。
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 1);
        synth.note_off(note(2, 0, 60));
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 0);
        assert_eq!(synth.state.next_oscillator_id, 0);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 全波形・レート・ベロシティの実効値は実CPUで照合する。
    fn every_waveform_has_expected_sustain_rms() {
        let measurements = [(44100, 44100), (48000, 48000), (96000, 96000)];
        for waveform in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for (rate, samples) in measurements {
                for velocity in [0.0, 0.25, 0.5, 1.0] {
                    let mut synth = unfiltered();
                    synth.state.voices[0].set_oscillator(0, waveform, 1.0, 1.0);
                    // 定常RMSの検証なので、立ち上がり・リリースだけ許容最短にする。
                    synth.state.voices[0].state.envelope_parameters.attack_ms = 1.0;
                    synth.state.voices[0].state.envelope_parameters.decay_ms = 1.0;
                    synth.state.voices[0].state.envelope_parameters.release_ms = 1.0;
                    synth.set_sample_rate(f64::from(rate));
                    assert!(synth.note_on(note(1, 0, 69), 0.0, velocity));
                    // attack/decayが終わったsustain区間のRMSを測る。
                    for _ in 0..(f64::from(rate) * 0.003).ceil() as usize {
                        synth.mix_sample();
                    }
                    let mut energy = 0.0;
                    for _ in 0..samples {
                        let sample = synth.mix_sample()[0];
                        assert!(sample.is_finite() && sample.abs() <= 1.0);
                        energy += sample * sample;
                    }
                    let rms = (energy / f64::from(samples)).sqrt();
                    // 残る倍音の振幅からRMSを求める。高次倍音の除去を音量で補償しない。
                    let expected =
                        crate::oscillator::reference::rms(waveform, 440.0 / f64::from(rate))
                            * crate::volume::gain(waveform, 1, 0.5)
                            * DEFAULT_ENVELOPE.sustain
                            * f64::from(velocity);
                    assert!(
                        (rms - expected).abs() < 1e-6,
                        "{waveform:?}, rate={rate}, velocity={velocity}, rms={rms}"
                    );
                    synth.note_off(note(1, 0, 69));
                    for _ in 0..(f64::from(rate) * 0.002).ceil() as usize {
                        synth.mix_sample();
                    }
                    for _ in 0..8 {
                        assert_eq!(synth.mix_sample()[0], 0.0);
                    }
                }
            }
        }
    }

    fn assert_enveloped_sine(synth: &mut Synthesizer, phase: &mut f64, amplitude: f64) {
        let frequency = synth.state.voices[0].state.oscillator_inputs[0].frequency_hz;
        let expected =
            amplitude * std::f64::consts::SQRT_2 * (std::f64::consts::TAU * *phase).sin();
        // 正弦波テーブルの補間誤差の上限を適用する。
        assert!((synth.mix_sample()[0] - expected).abs() < 3e-7);
        *phase = (*phase + frequency / 1000.0).fract();
    }

    #[test]
    fn retrigger_release_and_stop_preserve_amplitude_and_phase() {
        let mut synth = unfiltered();
        let mut phase = 0.0;
        synth.set_sample_rate(1000.0); // 1サンプル=1ms。
        synth.state.voices[0].set_oscillator(0, Waveform::Sine, 1.0, 1.0);
        synth.note_on(note(1, 0, 69), 0.0, 1.0);
        for i in 0..5 {
            assert_enveloped_sine(&mut synth, &mut phase, 0.05 * f64::from(i));
        }
        // attack中の0.25から、異なるベロシティの目標0.4まで10msで移行。
        synth.note_on(note(2, 0, 81), 0.0, 0.8);
        synth.note_off(note(1, 0, 69)); // 古いノートではreleaseしない。
        let peak = 0.5 * f64::from(0.8_f32);
        for i in 0..=10 {
            let expected = 0.25 + (peak - 0.25) * f64::from(i) / 10.0;
            assert_enveloped_sine(&mut synth, &mut phase, expected);
        }
        synth.note_off(note(2, 0, 81));
        let start = peak * 0.997; // decayを1ms進めた位置。
        for i in 0..=100 {
            assert_enveloped_sine(&mut synth, &mut phase, start * (1.0 - f64::from(i) / 100.0));
        }
        assert_eq!(synth.mix_sample()[0], 0.0);
        synth.note_on(note(3, 0, 69), 0.0, 1.0);
        assert_eq!(synth.mix_sample()[0], 0.0);
        synth.mix_sample();
        // stopの直後、サンプル処理前にNote Onが来ても古い音量を引き継がない。
        synth.stop();
        synth.note_on(note(4, 0, 69), 0.0, 1.0);
        assert_eq!(synth.mix_sample()[0], 0.0);
        synth.stop();
        assert_eq!(synth.mix_sample()[0], 0.0);
    }

    #[test]
    fn missing_note_ids_match_channel_and_pitch_and_invalid_notes_are_ignored() {
        let mut synth = unfiltered();
        let mut phase = 0.0;
        synth.set_sample_rate(1000.0);
        synth.state.voices[0].set_oscillator(0, Waveform::Sine, 1.0, 1.0);
        synth.note_on(note(-1, 2, 69), 0.0, 1.0);
        for i in 0..5 {
            assert_enveloped_sine(&mut synth, &mut phase, 0.05 * f64::from(i));
        }
        synth.note_off(note(-1, 3, 69));
        synth.note_off(note(-1, 2, 70));
        synth.note_on(note(10, 16, 69), 0.0, 1.0);
        synth.note_on(note(10, 0, 128), 0.0, 1.0);
        synth.note_on(note(10, 0, 69), f32::MAX, 1.0);
        synth.note_on(note(10, 0, 69), 0.0, f32::NAN);
        assert_enveloped_sine(&mut synth, &mut phase, 0.25);
        synth.note_off(note(-1, 2, 69));
        assert_enveloped_sine(&mut synth, &mut phase, 0.3);
        for _ in 0..100 {
            synth.mix_sample();
        }
        assert_eq!(synth.mix_sample()[0], 0.0);
    }
}

#[cfg(test)]
mod distortion_tests {
    use super::*;
    use crate::distortion::Distortion;

    fn settings() -> [f64; parameters::COUNT] {
        let mut p = parameters::sine_preset();
        for (i, v) in [
            (0, 3.0),
            (6, 7.0),
            (7, 25.0),
            (8, 4.0),
            (9, 2.0),
            (10, 1200.0),
            (12, 1.0),
            (42, 4.0),
            (31, 0.0),
            (32, 12.0),
        ] {
            p[i] = parameters::to_normalized(i, v);
        }
        p
    }
    #[test]
    #[cfg_attr(miri, ignore)] // Gain/Mixの数値総当たりは実CPUで照合する。
    fn adaa2_keeps_dry_exact_and_mixes_at_the_output_time() {
        for clipper in [0.0, 1.0] {
            let mut mixed = Synthesizer::new();
            let mut wet = Synthesizer::new();
            let mut dry = Synthesizer::new();
            let mut p = settings();
            p[31] = parameters::to_normalized(31, clipper);
            for synth in [&mut mixed, &mut wet, &mut dry] {
                synth.set_parameters(&p);
                for id in 0..4 {
                    synth.note_on(
                        Note {
                            id,
                            channel: 0,
                            pitch: 48 + id as i16 * 7,
                        },
                        0.0,
                        1.0,
                    );
                }
            }
            let mut bypass = p;
            bypass[42] = 0.0;
            dry.set_parameters(&bypass);
            let mut mixes =
                std::collections::VecDeque::from(vec![0.0; mixed.latency_samples() - 1]);
            for n in 0..500 {
                // Gainの段差・ランプ、Mixの急変。Mixは遅延前の値を使わない。
                p[32] = (n % 71) as f64 / 70.0;
                wet.set_parameters(&p);
                let mix = if n % 17 < 8 {
                    0.0
                } else {
                    (n % 31) as f64 / 30.0
                };
                let mut m = p;
                m[33] = mix;
                mixed.set_parameters(&m);
                let dry_sample = dry.generate_sample();
                let wet_sample = wet.generate_sample();
                let actual = mixed.generate_sample();
                mixes.push_back(mix);
                let mix = mixes.pop_front().unwrap();
                for ch in 0..2 {
                    let expected = dry_sample[ch] * (1.0 - mix) + wet_sample[ch] * mix;
                    assert_eq!(actual[ch], expected);
                    assert!(actual[ch].is_finite());
                }
            }
            mixed.stop();
            for _ in 0..150 {
                assert_eq!(mixed.generate_sample(), [0.0; 2]);
            }
        }
    }
    #[test]
    fn native_output_has_fixed_latency_and_stop_clears_history() {
        let mut synth = Synthesizer::new();
        let mut native = Synthesizer::new();
        let mut p = settings();
        p[42] = 0.0;
        synth.set_parameters(&p);
        native.set_parameters(&p);
        let mut expected =
            std::collections::VecDeque::from(vec![[0.0; 2]; synth.latency_samples()]);
        for n in 0..600 {
            for s in [&mut synth, &mut native] {
                if n == 0 || n == 7 {
                    s.note_on(
                        Note {
                            id: n,
                            channel: 0,
                            pitch: 60 + n as i16,
                        },
                        0.0,
                        1.0,
                    );
                }
                if n == 60 {
                    s.note_off(Note {
                        id: 0,
                        channel: 0,
                        pitch: 60,
                    });
                }
            }
            expected.push_back(native.mix_sample());
            assert_eq!(synth.generate_sample(), expected.pop_front().unwrap());
        }
        synth.stop();
        for _ in 0..150 {
            assert_eq!(synth.generate_sample(), [0.0; 2]);
        }
    }
    #[test]
    fn shared_distortion_follows_voice_sum_and_precedes_eq_and_master() {
        for clipper in [0.0, 1.0] {
            let mut synth = Synthesizer::new();
            let mut p = settings();
            p[31] = parameters::to_normalized(31, clipper);
            for (index, value) in [
                (13, -6.0),
                (20, 70.0),
                (21, 25.0),
                (33, 40.0),
                (34, 1.0),
                (36, 6.0),
                (38, -3.0),
                (41, 4.0),
            ] {
                p[index] = parameters::to_normalized(index, value);
            }
            synth.set_parameters(&p);
            let settings = parameters::distortion(&p, 0);
            let mut distortion = Distortion::new();
            let mut eq = Eq::new(44100.0, parameters::eq(&p));
            let gain = 10.0_f64.powf(parameters::to_plain(13, p[13]) / 20.0);
            let mut previous_dry = [0.0; 2];
            let mut padding = std::collections::VecDeque::from([[0.0; 2]; 2]);
            let mut limiter_delay = std::collections::VecDeque::from(vec![
                    [0.0; 2];
                    synth.latency_samples()
                        - crate::effects::LATENCY
                ]);
            let mut error_energy = 0.0;
            let mut signal_energy = 0.0;
            let mut references: [Synthesizer; 4] = std::array::from_fn(|_| {
                let mut s = Synthesizer::new();
                let mut p = p;
                p[8] = 0.0;
                s.set_parameters(&p);
                s
            });
            for (i, s) in references.iter_mut().enumerate() {
                let note = Note {
                    id: i as i32,
                    channel: 0,
                    pitch: 60 + i as i16,
                };
                s.note_on(note, 0.0, 1.0);
                synth.note_on(note, 0.0, 1.0);
            }
            for n in 0..800 {
                if n == 80 {
                    let note = Note {
                        id: 1,
                        channel: 0,
                        pitch: 61,
                    };
                    synth.note_off(note);
                    references[1].note_off(note);
                }
                if n == 100 {
                    p[8] = 0.0;
                    synth.set_parameters(&p);
                    for reference in &mut references[1..] {
                        reference.stop();
                    }
                }
                if n == 150 {
                    p[8] = parameters::to_normalized(8, 4.0);
                    synth.set_parameters(&p);
                    for (i, reference) in references.iter_mut().enumerate() {
                        let note = Note {
                            id: 10 + i as i32,
                            channel: 0,
                            pitch: 48 + i as i16 * 7,
                        };
                        synth.note_on(note, 0.0, 0.8);
                        reference.note_on(note, 0.0, 0.8);
                    }
                }
                // 独立した4音のSVF出力を合成し、共有履歴はPolyphony変更時も継続する。
                let dry = references
                    .iter_mut()
                    .map(|s| s.mix_sample())
                    .fold([0.0; 2], |sum, x| [sum[0] + x[0], sum[1] + x[1]]);
                let wet = distortion.process_sample(dry, &settings);
                let mixed = std::array::from_fn(|ch| {
                    previous_dry[ch] * (1.0 - settings.mix) + wet[ch] * settings.mix
                });
                previous_dry = dry;
                padding.push_back(mixed);
                limiter_delay.push_back(eq.process_sample(padding.pop_front().unwrap()));
                let expected = limiter_delay
                    .pop_front()
                    .unwrap()
                    .map(|sample| sample * gain);
                let actual = synth.generate_sample();
                for ch in 0..2 {
                    error_energy += (actual[ch] - expected[ch]).powi(2);
                    signal_energy += expected[ch].powi(2);
                }
            }
            assert!(signal_energy > 0.0);
            assert!(error_energy / signal_energy < 1e-8);
        }
    }
    #[test]
    fn live_switch_preserves_note_time_and_converges_to_the_requested_clipper() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let mut synth = Synthesizer::new();
            synth.set_sample_rate(rate);
            let mut p = settings();
            synth.set_parameters(&p);
            synth.note_on(
                Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                },
                0.0,
                1.0,
            );
            let delay = synth.latency_samples() - crate::effects::LATENCY;
            for target in [1.0, 0.0] {
                for _ in 0..200 + delay {
                    synth.generate_sample();
                }
                p[31] = parameters::to_normalized(31, target);
                synth.set_parameters(&p);
                for n in 0..66 + delay {
                    let out = synth.generate_sample();
                    assert!(out.into_iter().all(f64::is_finite));
                    if (34 + delay..36 + delay).contains(&n) {
                        assert_eq!(out, [0.0; 2]);
                    }
                    assert_eq!(synth.active_voice_count(), 1);
                }
            }
        }
    }
    #[test]
    fn clipper_switches_do_not_fade_or_reset_dry() {
        let mut synth = Synthesizer::new();
        let mut reference = Synthesizer::new();
        let mut p = settings();
        p[33] = 0.0;
        synth.set_parameters(&p);
        let mut bypass = p;
        bypass[42] = 0.0;
        reference.set_parameters(&bypass);
        for s in [&mut synth, &mut reference] {
            s.note_on(
                Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                },
                0.0,
                1.0,
            );
        }
        // MiriでもSoft/Hardの切替と66サンプルの遷移を通す。
        for n in 0..if cfg!(miri) { 1200 } else { 3000 } {
            if n % 300 == 0 {
                p[31] = ((n / 300) % 2) as f64;
                synth.set_parameters(&p);
            }
            assert_eq!(synth.generate_sample(), reference.generate_sample());
        }
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(test)]
use crate::distortion::LATENCY;
use crate::distortion::{ClipperType, Distortion, DistortionParameters};
use crate::envelope::DEFAULT_ENVELOPE;
use crate::eq::Eq;
use crate::oscillator::{PanningGains, panning_gains};
use crate::parameters;
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
    /// 共有ディストーション・EQの後に掛ける振幅倍率。Volume変更時だけ再計算する。
    master_gain: f64,
    /// 全Voice共通の定位係数。実発音数・有効なWidth・Panの変更時だけ更新する。
    panning_gains: PanningGains,
    /// ユニゾンを使わないOsc2/3の共通Pan係数。
    mono_gains: [f64; 2],
    /// 適用中のClipper。切り替え先はdistortion_parameters.clipperに保持する。
    processing: ClipperType,
    /// 32サンプルのfade-out、2のADAA履歴充填、32のfade-inの残り。
    transition: u8,
    /// 発音前・停止中の設定は即時反映する。
    started: bool,
    /// 停止・使用範囲拡大後の未使用枠を、次の有効サンプルで一度だけ更新する。
    unused_pending: bool,
    /// Dryは平均化を通さず、1サンプルの純遅延だけを掛ける。
    previous_dry: [f64; 2],
    /// バイパス時に既存のDryとMasterの適用時刻を維持する。
    previous_master_gain: f64,
    /// 共有ディストーション後、Master前のEQ。
    eq: Eq,
    /// 全Voice加算後のクリッパー。左右独立のADAA履歴を共有する。
    distortion: Distortion,
    /// 現在のクリップ設定。線形Gainは設定変更時に計算する。
    distortion_parameters: DistortionParameters,
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
                master_gain: 1.0,
                panning_gains: [[1.0; 2]; 7],
                mono_gains: [1.0; 2],
                processing: ClipperType::None,
                transition: 0,
                started: false,
                unused_pending: false,
                previous_dry: [0.0; 2],
                previous_master_gain: 1.0,
                eq: Eq::new(44100.0, parameters::eq(&parameters::defaults())),
                distortion: Distortion::new(),
                distortion_parameters: parameters::distortion(&parameters::defaults()),
            }),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) -> bool {
        if sample_rate.is_finite()
            && sample_rate >= crate::filter::MIN_SAMPLE_RATE
            && !self.state.eq.prepare(sample_rate)
        {
            return false;
        }
        self.state.sample_rate = sample_rate;
        true
    }

    pub fn parameters(&self) -> [f64; parameters::COUNT] {
        self.state.parameters
    }

    /// 設定は有効な枠の入力へ反映し、Polyphony増加時は追加した枠も同期する。
    pub fn set_parameters(&mut self, values: [f64; parameters::COUNT]) -> bool {
        if values == self.state.parameters {
            return false;
        }
        if values[13] != self.state.parameters[13] {
            self.state.master_gain = 10.0_f64.powf(parameters::to_plain(13, values[13]) / 20.0);
            // 全体の音量だけが変わる場合は、各Voiceへの設定配送を省く。
            if values[..13] == self.state.parameters[..13]
                && values[14..] == self.state.parameters[14..]
            {
                self.state.parameters = values;
                return true;
            }
        }
        if values[34..] != self.state.parameters[34..] {
            self.state.eq.set_parameters(parameters::eq(&values));
        }
        let previous = &self.state.parameters;
        // 有効範囲が広がるときは、休止していた枠へ現在の全設定を配送する。
        let expanded = values[8] > previous[8];
        let oscillator_changed =
            expanded || values[..8] != previous[..8] || values[12] != previous[12];
        let envelope_changed = expanded || values[14..20] != previous[14..20];
        if values[8] != previous[8] {
            let count = parameters::to_plain(8, values[8]) as usize;
            for voice in &mut self.state.voices[count..self.state.polyphony.max(count)] {
                voice.stop();
            }
            self.state.unused_pending |= count > self.state.polyphony;
            self.state.polyphony = count;
            self.state.used_voices = self.state.used_voices.min(count);
            self.state.next_oscillator_id %= count;
        }
        // 変更のないグループの時間換算（pow）なども省く。
        if oscillator_changed || envelope_changed {
            // Voice 0は常に有効で、適用済みの実単位を保持している。
            let mut envelopes = self.state.voices[0].envelope_parameters();
            for (update, first, envelope) in [oscillator_changed, envelope_changed]
                .into_iter()
                .zip([1, 14])
                .zip(&mut envelopes)
                .map(|((update, first), envelope)| (update, first, envelope))
            {
                if update {
                    // 初期実値と正規化値の往復で丸めが変わるため、初回は従来と同じ全換算。
                    if *envelope == DEFAULT_ENVELOPE {
                        *envelope = parameters::envelope(&values, first);
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
                        if values[first + i] != previous[first + i] {
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
                    voice.set_cutoff_envelope(cutoff_envelope, cutoff_amount);
                }
            }
        }
        if values[21] != previous[21] {
            self.state.mono_gains =
                panning_gains(1, 0.0, parameters::to_plain(21, values[21]) / 100.0)[0];
        }
        for (osc, volume) in parameters::OSCILLATOR_VOLUMES.into_iter().enumerate() {
            let wave = [0, 23, 26][osc];
            let tune = [None, Some(24), Some(27)][osc];
            if expanded
                || values[volume] != previous[volume]
                || values[wave] != previous[wave]
                || tune.is_some_and(|i| values[i] != previous[i])
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
        if expanded || values[29..31] != previous[29..31] {
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_phase_modulation([values[29] * 2.0, values[30] * 2.0]);
            }
        }
        if values[20..22] != previous[20..22]
            || values[6] != previous[6]
            || (values[7] == 0.0) != (previous[7] == 0.0)
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
            if count != old_count
                || values[21] != previous[21]
                || (count > 1 && values[20] != previous[20])
            {
                self.state.panning_gains = panning_gains(
                    count,
                    parameters::to_plain(20, values[20]) / 100.0,
                    parameters::to_plain(21, values[21]) / 100.0,
                );
            }
        }
        if values[31..34] != previous[31..34] {
            self.state.distortion_parameters = parameters::distortion(&values);
        }
        if expanded || values[9..11] != previous[9..11] {
            let filter_type = parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize];
            let cutoff_cents = parameters::to_plain(10, values[10]);
            let resonance_q = parameters::to_plain(11, values[11]);
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_filter(filter_type, cutoff_cents, resonance_q);
            }
        } else if values[11] != previous[11] {
            let resonance_q = parameters::to_plain(11, values[11]);
            for voice in &mut self.state.voices[..self.state.polyphony] {
                voice.set_resonance(resonance_q);
            }
        }
        let clipper_changed = values[31] != previous[31];
        self.state.parameters = values;
        if clipper_changed && !self.state.started {
            self.state.processing = self.state.distortion_parameters.clipper;
            self.state.distortion.reset();
        }
        true
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
        self.state.previous_dry = [0.0; 2];
        self.state.previous_master_gain = self.state.master_gain;
        self.state.eq.reset();
        self.state.distortion.reset();
        self.state.started = false;
        self.state.used_voices = 0;
        self.state.unused_pending = true;
        self.state.transition = 0;
        self.state.processing = self.state.distortion_parameters.clipper;
        for voice in &mut self.state.voices {
            voice.stop();
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
        if output.len() <= 1
            || self.state.transition != 0
            || self.state.processing != self.state.distortion_parameters.clipper
        {
            for out in output {
                *out = self.generate_sample();
            }
            return;
        }
        if self.state.unused_pending {
            self.update_unused_sample();
        }
        self.state.started = true;
        let clipper = self.state.processing;
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
        if clipper == ClipperType::None {
            self.finish_dry_samples(output);
        } else {
            let distortion = self.state.distortion_parameters;
            for chunk in output.chunks_mut(32) {
                let mut wet = [[0.0; 2]; 32];
                self.state
                    .distortion
                    .process_samples(chunk, &mut wet[..chunk.len()], &distortion);
                for (out, wet) in chunk.iter_mut().zip(&wet) {
                    *out = self.finish_sample(*out, *wet, distortion.mix, 1.0);
                }
            }
        }
    }

    pub fn generate_sample(&mut self) -> [f64; 2] {
        if self.state.unused_pending {
            self.update_unused_sample();
        }
        self.state.started = true;
        if self.state.transition == 0
            && self.state.processing != self.state.distortion_parameters.clipper
        {
            self.state.transition = 66;
        }
        if self.state.transition == 34 {
            self.state.processing = self.state.distortion_parameters.clipper;
            self.state.distortion.reset();
        }
        let clipper = self.state.processing;
        let remaining = self.state.transition;
        self.state.transition = remaining.saturating_sub(1);
        let dry = self.mix_sample();
        if clipper == ClipperType::None {
            return self.finish_sample(dry, [0.0; 2], 0.0, 1.0);
        }
        let mut distortion = self.state.distortion_parameters;
        distortion.clipper = clipper;
        let wet = self.state.distortion.process_sample(dry, &distortion);
        let gain = match remaining {
            0 => 1.0,
            1..=32 => (32 - remaining) as f64 / 32.0,
            33..=34 => 0.0,
            _ => (remaining - 35) as f64 / 32.0,
        };
        // DistortionのDryとWetを合成し、全体エフェクトへ渡す。
        self.finish_sample(dry, wet, distortion.mix, gain)
    }

    /// Offの後処理を分離し、EQ無効の系列では係数と分岐をループ外へ出す。
    #[inline(never)]
    fn finish_dry_samples(&mut self, output: &mut [[f64; 2]]) {
        for out in output {
            *out = self.finish_sample(*out, [0.0; 2], 0.0, 1.0);
        }
    }

    /// DistortionのDry遅延後に全体エフェクトを処理し、Masterを適用する。
    #[inline]
    fn finish_sample(&mut self, dry: [f64; 2], wet: [f64; 2], mix: f64, fade: f64) -> [f64; 2] {
        let dry = std::mem::replace(&mut self.state.previous_dry, dry);
        let gain = self.state.master_gain;
        let previous_gain = std::mem::replace(&mut self.state.previous_master_gain, gain);
        if self.state.eq.enabled() {
            let input = std::array::from_fn(|ch| dry[ch] * (1.0 - mix) + wet[ch] * mix * fade);
            self.state.eq.process_sample(input).map(|v| v * gain)
        } else {
            std::array::from_fn(|ch| {
                (dry[ch] * previous_gain) * (1.0 - mix) + (wet[ch] * gain) * mix * fade
            })
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::DEFAULT_ENVELOPE;
    use crate::oscillator::Waveform;

    // 既存の発振器・ノート処理の期待値はフィルタをバイパスして検証する。
    fn unfiltered() -> Synthesizer {
        let mut synth = Synthesizer::new();
        let mut values = parameters::defaults();
        values[9] = 0.0;
        synth.set_parameters(values);
        synth
    }

    fn note(id: i32, channel: i16, pitch: i16) -> Note {
        Note { id, channel, pitch }
    }

    #[test]
    fn unused_stop_is_consumed_only_after_a_valid_sample() {
        for resized in [false, true] {
            for advance in 0..6 {
                let mut synth = Synthesizer::new();
                let mut values = parameters::sine_preset();
                values[5] = parameters::to_normalized(5, 100.0);
                values[8] = parameters::to_normalized(8, if resized { 2.0 } else { 1.0 });
                synth.set_parameters(values);
                synth.set_sample_rate(48000.0);
                if resized {
                    // 2枠目を停止して外し、再拡大後も未使用のままにする。
                    values[8] = parameters::to_normalized(8, 1.0);
                    synth.set_parameters(values);
                    synth.generate_samples(&mut [[0.0; 2]; 8]);
                    values[8] = parameters::to_normalized(8, 2.0);
                    synth.set_parameters(values);
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
                let mut output = [[0.0; 2]; 64];
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
            for (index, value) in [(5, 1.), (12, phase as f64), (32, 18.), (33, 65.)] {
                values[index] = parameters::to_normalized(index, value);
            }
            let mut serial = Synthesizer::new();
            serial.set_sample_rate(48000.);
            serial.set_parameters(values);
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
                    synth.set_parameters(values);
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
                    assert!(output[..count].iter().all(|sample| *sample == [0.; 2]));
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
            synth.set_parameters(values);
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
            synth.set_parameters(values);
            for id in 0..4 {
                assert!(synth.note_on(note(id, 0, 60 + id as i16), 0.0, 0.8));
            }
            for _ in 0..128 {
                synth.generate_sample();
            }
            let mut limited = values;
            limited[8] = 0.0;
            synth.set_parameters(limited);
        }
        reference.set_parameters(values);
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
        ] {
            values[i] = parameters::to_normalized(i, plain);
        }
        reference.set_parameters(values);
        let mut limited = values;
        limited[8] = 0.0;
        delayed.set_parameters(limited);
        delayed.set_parameters(values);
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
        synth.set_parameters(values);
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
        synth.set_parameters(values);
        assert_eq!(synth.active_voice_count(), 2);
        synth.stop();
        assert_eq!(synth.active_voice_count(), 0);
        values[8] = 1.0;
        synth.set_parameters(values);
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
                synth.set_parameters(values);
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
                            synth.set_parameters(values);
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
            synth.set_parameters(values);
            assert_eq!(synth.mix_sample(), [0.0; 2]);
            // 保存復元と同じ、発音前の設定配送が新しい全枠へ適用される。
            let mut restored = Synthesizer::new();
            restored.set_sample_rate(48000.0);
            restored.set_parameters(values);
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
        for index in (0..34).filter(|&i| i != 8 && i != 12) {
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
                    synth.set_parameters(values);
                    // 設定を配送した後に使用枠を増やす。
                    values[8] = parameters::to_normalized(8, f64::from(count));
                    synth.set_parameters(values);
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
                    reference.note_on(note(0, 0, 69), 0.0, 1.0);
                    for id in 0..count {
                        synth.note_on(note(id, 0, 69), 0.0, 1.0);
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
        synth.set_parameters(values);
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
        reference.set_parameters(values);
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
            synth.set_parameters(values);
            let mut references: [Synthesizer; 2] = std::array::from_fn(|_| {
                let mut single = Synthesizer::new();
                let mut values = values;
                values[8] = 0.0;
                single.set_parameters(values);
                single
            });
            let mut delayed_expected = 0.0;
            for n in 0..96 {
                let db = match n {
                    0..16 => 0.0,
                    16..32 => -48.0,
                    32..48 => -12.3,
                    48..72 => 12.0,
                    _ => 0.0,
                };
                values[13] = parameters::to_normalized(13, db);
                synth.set_parameters(values);
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
                    synth.set_parameters(values);
                    references[1].stop();
                }
                let expected: f64 = references.iter_mut().map(|s| s.mix_sample()[0]).sum();
                assert!((synth.generate_sample()[0] - delayed_expected).abs() < 1e-12);
                delayed_expected = expected * 10.0_f64.powf(db / 20.0);
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
        synth.set_parameters(values);
        let mut references: [Synthesizer; 3] = std::array::from_fn(|_| {
            let mut single = polyphonic(1);
            let mut settings = values;
            settings[8] = 0.0;
            single.set_parameters(settings);
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
        synth.set_parameters(values);
        assert_eq!(synth.state.next_oscillator_id, 1); // 3 % 2
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 2);
        values[8] = parameters::to_normalized(8, 4.0);
        synth.set_parameters(values);
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 2); // 除外したノートは復活しない。
        synth.note_on(note(4, 0, 60), 0.0, 1.0); // 1番を上書き。
        synth.note_on(note(5, 0, 60), 0.0, 1.0); // 2番を再使用。
        synth.mix_sample();
        assert_eq!(synth.active_voice_count(), 3);
        // サンプル処理を挟まず減少→増加→再割り当てしても古い振幅を引き継がない。
        values[8] = 0.0;
        synth.set_parameters(values);
        values[8] = parameters::to_normalized(8, 4.0);
        values[1] = parameters::to_normalized(1, 10.0);
        synth.set_parameters(values);
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
            (31, 1.0),
            (32, 12.0),
        ] {
            p[i] = parameters::to_normalized(i, v);
        }
        p
    }
    #[test]
    #[cfg_attr(miri, ignore)] // Gain/Mixの数値総当たりは実CPUで照合する。
    fn adaa2_keeps_dry_exact_and_mixes_at_the_output_time() {
        for clipper in [1.0, 2.0] {
            let mut mixed = Synthesizer::new();
            let mut wet = Synthesizer::new();
            let mut dry = Synthesizer::new();
            let mut p = settings();
            p[31] = parameters::to_normalized(31, clipper);
            for synth in [&mut mixed, &mut wet, &mut dry] {
                synth.set_parameters(p);
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
            bypass[31] = 0.0;
            dry.set_parameters(bypass);
            for n in 0..500 {
                // Gainの段差・ランプ、Mixの急変。Mixは遅延前の値を使わない。
                p[32] = (n % 71) as f64 / 70.0;
                wet.set_parameters(p);
                let mix = if n % 17 < 8 {
                    0.0
                } else {
                    (n % 31) as f64 / 30.0
                };
                let mut m = p;
                m[33] = mix;
                mixed.set_parameters(m);
                let dry_sample = dry.generate_sample();
                let wet_sample = wet.generate_sample();
                let actual = mixed.generate_sample();
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
        p[31] = 0.0;
        synth.set_parameters(p);
        native.set_parameters(p);
        let mut expected = std::collections::VecDeque::from(vec![[0.0; 2]; LATENCY]);
        for n in 0..200 {
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
        for clipper in [1.0, 2.0] {
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
            synth.set_parameters(p);
            let settings = parameters::distortion(&p);
            let mut distortion = Distortion::new();
            let mut eq = Eq::new(44100.0, parameters::eq(&p));
            let gain = 10.0_f64.powf(parameters::to_plain(13, p[13]) / 20.0);
            let mut previous_dry = [0.0; 2];
            let mut error_energy = 0.0;
            let mut signal_energy = 0.0;
            let mut references: [Synthesizer; 4] = std::array::from_fn(|_| {
                let mut s = Synthesizer::new();
                let mut p = p;
                p[8] = 0.0;
                s.set_parameters(p);
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
            for n in 0..220 {
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
                    synth.set_parameters(p);
                    for reference in &mut references[1..] {
                        reference.stop();
                    }
                }
                if n == 150 {
                    p[8] = parameters::to_normalized(8, 4.0);
                    synth.set_parameters(p);
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
                let expected = eq.process_sample(mixed).map(|sample| sample * gain);
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
            synth.set_parameters(p);
            synth.note_on(
                Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                },
                0.0,
                1.0,
            );
            for target in [2.0, 1.0] {
                for _ in 0..200 {
                    synth.generate_sample();
                }
                p[31] = parameters::to_normalized(31, target);
                synth.set_parameters(p);
                for n in 0..66 {
                    let out = synth.generate_sample();
                    assert!(out.into_iter().all(f64::is_finite));
                    if (32..34).contains(&n) {
                        assert_eq!(out, [0.0; 2]);
                    }
                    assert_eq!(synth.active_voice_count(), 1);
                }
                assert_eq!(
                    synth.state.processing,
                    parameters::CLIPPER_TYPES[target as usize]
                );
                assert_eq!(synth.state.transition, 0);
            }
        }
    }
    #[test]
    fn clipper_switches_do_not_fade_or_reset_dry() {
        let mut synth = Synthesizer::new();
        let mut reference = Synthesizer::new();
        let mut p = settings();
        p[33] = 0.0;
        synth.set_parameters(p);
        let mut bypass = p;
        bypass[31] = 0.0;
        reference.set_parameters(bypass);
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
        // MiriでもNone→Soft→Hard→Noneの各切替と66サンプルの遷移を通す。
        for n in 0..if cfg!(miri) { 1200 } else { 3000 } {
            if n % 300 == 0 {
                p[31] = ((n / 300) % 3) as f64 / 2.0;
                synth.set_parameters(p);
            }
            assert_eq!(synth.generate_sample(), reference.generate_sample());
            if n % 300 == 299 {
                assert_eq!(
                    synth.state.processing,
                    parameters::CLIPPER_TYPES[parameters::to_plain(31, p[31]) as usize]
                );
                assert_eq!(synth.state.transition, 0);
            }
        }
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::envelope::{DEFAULT_ENVELOPE, Envelope, EnvelopeParameters};
use crate::filter::{Filter, FilterParameters, FilterType, MIN_SAMPLE_RATE};
use crate::oscillator::{Oscillator, OscillatorInput, PanningGains, PhaseMode, Waveform};
use crate::{parameters, volume};

/// 全波形共通のvelocityが1のときの基準実効値（RMS）。
const BASE_RMS_AMPLITUDE: f64 = 0.5;

#[derive(Clone, Copy)]
pub struct Note {
    /// ホストのnoteId。両ノートにIDがあればこれで照合し、どちらかが-1ならchannelとpitchで照合する。
    pub id: i32,
    /// イベント入力チャンネル（0..=15）。IDが未提供の場合のノート照合に使う。
    pub channel: i16,
    /// 12平均律のノート番号（0..=127）。周波数計算とID未提供時のノート照合に使う。
    pub pitch: i16,
}

/// 同時発音の1枠。入力・内部状態・ノートの対応を他の枠から独立させる。
#[cfg_attr(test, derive(Clone))]
pub struct VoiceState {
    /// Osc1・Osc2・Osc3の定常入力。Osc1だけがユニゾン設定を持つ。
    pub(super) oscillator_inputs: [OscillatorInput; 3],
    /// 発振器ごとに独立した位相・乱数の状態。
    oscillators: [Oscillator; 3],
    /// 各発振器へ定常入力を反映済みなら対応ビットが1。設定・ノート変更時に失効する。
    oscillators_prepared: u8,
    /// Volの線形倍率。−∞ dBでは0。
    oscillator_volumes: [f64; 3],
    /// 波形・ユニゾン補正とVolを合わせた倍率。設定変更時に計算する。
    oscillator_gains: [f64; 3],
    /// Osc2→Osc1、Osc3→Osc2の位相変調量（0〜2周期）。
    phase_depths: [f64; 2],
    /// ノート基音に掛けるOsc2/3のTune倍率。Osc1は1。
    tune_ratios: [f64; 3],
    /// ベンドと各Tuneを適用する前の現在のノート周波数（Hz）。
    note_hz: f64,
    /// 直近のNote Onで指定された移行先のノート周波数（Hz）。
    target_hz: f64,
    /// Polyphony=1のときだけ親が設定する移行時間（ms）。0は即時変更。
    portamento_ms: f64,
    /// 進行中の移行の残り時間（ms）。0なら周波数を保持する。
    glide_remaining_ms: f64,
    /// cent上の線形移行を表す1サンプルの周波数倍率。
    glide_ratio: f64,
    /// ホストのピッチベンドとPitch幅から親が換算した全発振器共通の倍率。
    pitch_ratio: f64,
    /// ノートを保持している間true。Note Offでは共通AHDSRのreleaseへ移る。
    pub(super) gate: bool,
    /// Note Onの世代。3発振器へ同じ番号を渡す。
    note_generation: u64,
    /// 両Envelopeへ反映済みの有効なレート。生成時に変わった場合だけ再検証する。
    sample_rate: f64,
    /// ベロシティを反映した共通AHDSRの目標実効振幅。
    amplitude: f64,
    /// 現在のノートのベロシティ（0〜1）。release中も保持する。
    velocity: f64,
    /// 音量・カットオフAmountそれぞれのベロシティ感度（0〜1）。
    velocity_sensitivity: [f64; 2],
    /// 発振器の加算出力へ適用する音量AHDSRの設定。
    pub(super) envelope_parameters: EnvelopeParameters,
    /// 3発振器に共通の音量AHDSRと時間履歴。
    envelope: Envelope,
    /// Note Offの照合対象。発振器へノート情報を渡さない。
    note: Option<Note>,
    /// Voiceの最終段のフィルタ。ノートオフ後も残響を処理する。
    filter: Filter,
    /// 選択中の応答と段数。Noneでは完全にバイパスする。
    filter_type: FilterType,
    /// ノート周波数に乗じる倍率。centからの換算は設定変更時に行う。
    cutoff_ratio: f64,
    /// 各段へ渡す現在のQ。積分器の履歴とは独立した入力。
    resonance_q: f64,
    /// 音量から独立したカットオフ用AHDSRの設定。
    cutoff_envelope_parameters: EnvelopeParameters,
    /// このVoiceのカットオフ用正規化エンベロープと時間履歴。
    cutoff_envelope: Envelope,
    /// エンベロープ値1に対応するカットオフ変化量（−4800〜4800 cent）。
    cutoff_amount_cents: f64,
    /// ベロシティと感度を反映したAmount。ノート・設定変更時だけ計算する。
    velocity_cutoff_amount: f64,
    /// 直近の変調量centと周波数倍率。同じ値では指数計算を省く。
    cutoff_modulation: (f64, f64),
}

/// Voiceの処理を担当し、専用のStateを所有するコンポーネント。
#[cfg_attr(test, derive(Clone))]
pub struct Voice {
    /// このコンポーネントの内部状態。
    pub(super) state: VoiceState,
}

impl Voice {
    pub fn new() -> Self {
        let values = parameters::defaults();
        let mut envelope = Envelope::new();
        envelope.set_input(&DEFAULT_ENVELOPE, MIN_SAMPLE_RATE, 0.0);
        let mut cutoff_envelope = Envelope::new();
        cutoff_envelope.set_input(&DEFAULT_ENVELOPE, MIN_SAMPLE_RATE, 1.0);
        Self {
            state: VoiceState {
                oscillator_inputs: [OscillatorInput {
                    waveform: Waveform::Sine,
                    phase_mode: PhaseMode::Keep,
                    unison_voices: 1,
                    detune_cents: 0.0,
                    frequency_hz: 440.0,
                    phase_modulation: 0.0,
                    phase_modulated: false,
                    active: false,
                    note_generation: 0,
                }; 3],
                oscillators: std::array::from_fn(|_| Oscillator::new()),
                oscillators_prepared: 0,
                oscillator_volumes: [1.0, 0.0, 0.0],
                oscillator_gains: [std::f64::consts::SQRT_2, 0.0, 0.0],
                phase_depths: [0.0; 2],
                tune_ratios: [1.0; 3],
                note_hz: 440.0,
                target_hz: 440.0,
                portamento_ms: 0.0,
                glide_remaining_ms: 0.0,
                glide_ratio: 1.0,
                pitch_ratio: 1.0,
                gate: false,
                note_generation: 0,
                sample_rate: MIN_SAMPLE_RATE,
                amplitude: 0.0,
                velocity: 0.0,
                velocity_sensitivity: [values[130], values[131]],
                envelope_parameters: DEFAULT_ENVELOPE,
                envelope,
                note: None,
                filter: Filter::new(),
                filter_type: parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize],
                cutoff_ratio: 2.0_f64.powf(parameters::to_plain(10, values[10]) / 1200.0),
                resonance_q: parameters::to_plain(11, values[11]),
                cutoff_envelope_parameters: DEFAULT_ENVELOPE,
                cutoff_envelope,
                cutoff_amount_cents: 0.0,
                velocity_cutoff_amount: 0.0,
                cutoff_modulation: (0.0, 1.0),
            },
        }
    }

    pub fn note_on(&mut self, note: Note, tuning: f32, velocity: f32) -> bool {
        if !(0..16).contains(&note.channel)
            || !(0..128).contains(&note.pitch)
            || !tuning.is_finite()
            || !(0.0..=1.0).contains(&velocity)
        {
            return false;
        }
        let frequency_hz =
            440.0 * 2.0_f64.powf((f64::from(note.pitch) - 69.0 + f64::from(tuning) / 100.0) / 12.0);
        if !frequency_hz.is_finite() {
            return false;
        }
        self.state.oscillators_prepared = 0;
        self.state.note_generation = self.state.note_generation.wrapping_add(1);
        self.state.target_hz = frequency_hz;
        self.state.glide_remaining_ms = if self.is_active()
            && self.state.envelope.is_active()
            && self.state.note_hz > 0.0
            && frequency_hz > 0.0
            && self.state.note_hz != frequency_hz
        {
            self.state.portamento_ms
        } else {
            0.0
        };
        if self.state.glide_remaining_ms == 0.0 {
            self.state.note_hz = frequency_hz;
        } else {
            self.update_glide_ratio();
        }
        self.refresh_pitch();
        for input in &mut self.state.oscillator_inputs {
            input.note_generation = self.state.note_generation;
        }
        self.state.velocity = f64::from(velocity);
        self.refresh_velocity();
        self.state.envelope.note_on();
        self.state.cutoff_envelope.note_on();
        self.state.gate = true;
        self.state.note = Some(note);
        true
    }

    /// 時間は次のNote Onに適用する。無効化時は進行中の移行も直ちに完了する。
    pub fn set_portamento(&mut self, milliseconds: f64) {
        self.state.portamento_ms = milliseconds;
        if milliseconds == 0.0 && self.state.glide_remaining_ms > 0.0 {
            self.state.glide_remaining_ms = 0.0;
            self.state.note_hz = self.state.target_hz;
            self.refresh_pitch();
        }
    }

    pub fn set_pitch_ratio(&mut self, ratio: f64) {
        if self.state.pitch_ratio != ratio {
            self.state.pitch_ratio = ratio;
            self.refresh_pitch();
        }
    }

    fn refresh_pitch(&mut self) {
        let base = (self.state.note_hz * self.state.pitch_ratio).min(f64::MAX);
        for (input, ratio) in self
            .state
            .oscillator_inputs
            .iter_mut()
            .zip(self.state.tune_ratios)
        {
            input.frequency_hz = (base * ratio).min(f64::MAX);
        }
        self.state.oscillators_prepared = 0;
    }

    fn update_glide_ratio(&mut self) {
        let samples = self.state.glide_remaining_ms * self.state.sample_rate / 1000.0;
        self.state.glide_ratio =
            ((self.state.target_hz.log2() - self.state.note_hz.log2()) / samples).exp2();
    }

    fn update_pitch_sample(&mut self) {
        if self.state.glide_remaining_ms > 0.0 {
            self.state.glide_remaining_ms =
                (self.state.glide_remaining_ms - 1000.0 / self.state.sample_rate).max(0.0);
            if self.state.glide_remaining_ms < 1e-9 {
                self.state.glide_remaining_ms = 0.0;
                self.state.note_hz = self.state.target_hz;
            } else {
                self.state.note_hz = (self.state.note_hz * self.state.glide_ratio).min(f64::MAX);
            }
            self.refresh_pitch();
        }
    }

    /// 親が検証した感度を反映し、発音中のノートにも適用する。
    pub fn set_velocity_sensitivity(&mut self, sensitivity: [f64; 2]) {
        self.state.velocity_sensitivity = sensitivity;
        self.refresh_velocity();
    }

    fn refresh_velocity(&mut self) {
        self.state.amplitude = BASE_RMS_AMPLITUDE
            * (1.0 - self.state.velocity_sensitivity[0] * (1.0 - self.state.velocity));
        self.state.envelope.set_input(
            &self.state.envelope_parameters,
            self.state.sample_rate,
            self.state.amplitude,
        );
        self.refresh_cutoff_amount();
    }

    fn refresh_cutoff_amount(&mut self) {
        self.state.velocity_cutoff_amount = self.state.cutoff_amount_cents
            * (1.0 - self.state.velocity_sensitivity[1] * (1.0 - self.state.velocity));
    }

    pub fn note_off(&mut self, note: Note) {
        if self.state.note.is_some_and(|current| {
            if current.id != -1 && note.id != -1 {
                current.id == note.id
            } else {
                current.channel == note.channel && current.pitch == note.pitch
            }
        }) {
            self.state.gate = false;
            self.state.envelope.note_off();
            self.state.cutoff_envelope.note_off();
            self.state.note = None;
        }
    }

    /// ノート保持中または音量AHDSRのrelease中のVoiceを数える。
    pub fn is_active(&self) -> bool {
        self.state.gate || self.state.oscillator_inputs[0].active
    }

    pub fn stop(&mut self) {
        self.state.glide_remaining_ms = 0.0;
        self.state.oscillators_prepared = 0;
        // releaseとは区別し、次サンプルで即時消音する。位相は保持する。
        self.state.gate = false;
        self.state.envelope.stop();
        self.state.cutoff_envelope.stop();
        self.state.note = None;
        self.state.filter.reset();
        for input in &mut self.state.oscillator_inputs {
            input.active = false;
        }
    }

    /// 通常の停止で保持する位相と乱数列を、明示的なDSPリセットで初期化する。
    pub fn reset_oscillators(&mut self) {
        self.state.oscillators_prepared = 0;
        for oscillator in &mut self.state.oscillators {
            oscillator.reset();
        }
    }

    pub fn set_filter(&mut self, filter_type: FilterType, cutoff_cents: f64, resonance_q: f64) {
        self.state.filter_type = filter_type;
        self.state.cutoff_ratio = 2.0_f64.powf(cutoff_cents / 1200.0);
        self.state.resonance_q = resonance_q;
    }

    /// Qだけが変わるときは、カットオフの指数換算と書込みを省く。
    pub fn set_resonance(&mut self, resonance_q: f64) {
        self.state.resonance_q = resonance_q;
    }

    pub fn set_cutoff_envelope(&mut self, parameters: EnvelopeParameters, amount_cents: f64) {
        self.state.cutoff_envelope_parameters = parameters;
        self.state
            .cutoff_envelope
            .set_input(&parameters, self.state.sample_rate, 1.0);
        self.state.cutoff_amount_cents = amount_cents;
        self.refresh_cutoff_amount();
    }

    /// 親の差分換算に使う、適用済みの音量・カットオフAHDSR設定。
    pub fn envelope_parameters(&self) -> [EnvelopeParameters; 2] {
        [
            self.state.envelope_parameters,
            self.state.cutoff_envelope_parameters,
        ]
    }

    pub fn set_parameters(
        &mut self,
        waveform: Waveform,
        envelope: EnvelopeParameters,
        unison: u32,
        detune: f64,
        phase_mode: PhaseMode,
    ) {
        let input = &self.state.oscillator_inputs[0];
        if input.phase_mode != phase_mode {
            self.state.oscillators_prepared = 0;
        } else if input.waveform != waveform
            || input.unison_voices != unison
            || input.detune_cents != detune
        {
            self.state.oscillators_prepared &= !1;
        }
        self.state.oscillator_inputs[0].waveform = waveform;
        self.state.envelope_parameters = envelope;
        self.state
            .envelope
            .set_input(&envelope, self.state.sample_rate, self.state.amplitude);
        self.state.oscillator_inputs[0].unison_voices = unison;
        self.state.oscillator_inputs[0].detune_cents = detune;
        self.refresh_volume(0);
        for input in &mut self.state.oscillator_inputs {
            input.phase_mode = phase_mode;
        }
    }

    /// 設定変更時だけ親が換算したTune倍率・音量を配送する。
    pub fn set_oscillator(&mut self, index: usize, waveform: Waveform, tune_ratio: f64, gain: f64) {
        self.state.oscillator_volumes[index] = gain;
        self.state.tune_ratios[index] = tune_ratio;
        let base_hz = self.state.oscillator_inputs[0].frequency_hz;
        let frequency_hz = (base_hz * tune_ratio).min(f64::MAX);
        let input = &mut self.state.oscillator_inputs[index];
        if input.waveform != waveform || input.frequency_hz != frequency_hz {
            self.state.oscillators_prepared &= !(1 << index);
        }
        input.waveform = waveform;
        input.frequency_hz = frequency_hz;
        self.refresh_volume(index);
    }

    fn refresh_volume(&mut self, index: usize) {
        let input = &self.state.oscillator_inputs[index];
        self.state.oscillator_gains[index] = volume::gain(
            input.waveform,
            input.rendered_voices(),
            self.state.oscillator_volumes[index],
        );
    }

    /// 親が検証したOsc2・Osc3のMod設定を配送する。
    pub fn set_phase_modulation(&mut self, depths: [f64; 2]) {
        for (i, (&old, &new)) in self.state.phase_depths.iter().zip(&depths).enumerate() {
            if (old != 0.0) != (new != 0.0) {
                self.state.oscillators_prepared &= !(1 << i);
            }
        }
        self.state.phase_depths = depths;
    }

    /// Voiceの3発振器・音量AHDSR・SVFまでをホストレートで処理する。
    #[inline(never)] // 大きい1サンプル本体をSynthesizerへ重複展開しない。
    pub fn generate_sample(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
    ) -> [f64; 2] {
        self.generate_one_sample::<false>(sample_rate, gains, mono_gains)
    }

    /// 定常入力が同じ区間の先頭で必要な準備を済ませ、残りのサンプルを生成する。
    pub fn generate_samples(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
        output: &mut [[f64; 2]],
    ) {
        if let Some((first, rest)) = output.split_first_mut() {
            *first = self.generate_sample(sample_rate, gains, mono_gains);
            // 先頭を生成した時点で既に停止済みなら、後続の入力・時計も変化しない。
            // 更新直後にreleaseが完了した区間は、active表示の同期を通常経路へ任せる。
            if matches!(self.state.filter_type, FilterType::None)
                && !self.state.oscillator_inputs[0].active
                && !self.state.envelope.is_active()
                && !self.state.cutoff_envelope.is_active()
            {
                std::hint::cold_path();
                rest.fill([0.0; 2]);
                return;
            }
            // 移行中は周波数がサンプルごとに変わるため、定常入力専用の経路を使わない。
            if self.state.oscillators_prepared != 0b111 {
                for value in rest {
                    *value = self.generate_sample(sample_rate, gains, mono_gains);
                }
                return;
            }
            if self.state.oscillator_inputs[0].phase_modulated
                || self.state.oscillator_inputs[1].phase_modulated
            {
                self.generate_pipeline_samples(sample_rate, gains, mono_gains, rest);
                return;
            }
            for value in rest {
                *value = self.generate_one_sample::<true>(sample_rate, gains, mono_gains);
            }
        }
    }

    /// 位相変調の一方向の依存関係に従い、発振器ごとに定常区間を生成する。
    #[inline(never)]
    fn generate_pipeline_samples(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
        output: &mut [[f64; 2]],
    ) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            output.fill([0.0; 2]);
            return;
        }
        // この区間で発音する発振器は、呼出し元の通常生成で準備済み。
        for rest in output.chunks_mut(32) {
            let mut amplitudes = [0.0; 32];
            let mut cutoffs = [0.0; 32];
            let mut active_count = 0;
            let mut modulation = [0.0; 32];
            for n in 0..rest.len() {
                amplitudes[n] = self.state.envelope.generate_sample();
                cutoffs[n] = self.state.cutoff_envelope.generate_sample()
                    * self.state.velocity_cutoff_amount;
                active_count += usize::from(self.state.envelope.is_active());
                self.state.envelope.update_sample();
                self.state.cutoff_envelope.update_sample();
            }
            rest.fill([0.0; 2]);
            let mono = [*mono_gains; 7];
            let depths = [0.0, self.state.phase_depths[0], self.state.phase_depths[1]];
            // 変調配列をOsc3→Osc2→Osc1の順に上書きし、同じ順で音声へ加算する。
            for i in (0..3).rev() {
                let gain = self.state.oscillator_gains[i];
                let pans = if i == 0 { gains } else { &mono };
                let input = &mut self.state.oscillator_inputs[i];
                if gain == 0.0 && depths[i] == 0.0 {
                    for phase_modulation in &mut modulation[..active_count] {
                        input.phase_modulation = *phase_modulation;
                        // SAFETY: 区間の先頭で同じ定常入力を反映済み。
                        // activeはtrue→falseだけ、変調は有限のi32範囲内。
                        unsafe {
                            self.state.oscillators[i].generate_prepared_sample::<false>(input);
                        }
                        *phase_modulation = 0.0;
                    }
                } else {
                    for n in 0..active_count {
                        input.phase_modulation = modulation[n];
                        // SAFETY: 上と同じ準備済みの定常入力と発音期間を使う。
                        let current = unsafe {
                            self.state.oscillators[i].generate_prepared_sample::<true>(input)
                        };
                        modulation[n] = current.values[0] * depths[i];
                        if gain != 0.0 {
                            let audio = volume::process_sample(
                                &current.values[..current.count],
                                pans,
                                gain,
                            );
                            rest[n][0] += audio[0];
                            rest[n][1] += audio[1];
                        }
                    }
                }
                // 定常区間内でactiveはtrue→falseだけ。次Note Onで履歴を再準備する。
                if active_count < rest.len() {
                    input.active = false;
                }
            }
            if matches!(self.state.filter_type, FilterType::None) {
                for (sample, amplitude) in rest.iter_mut().zip(amplitudes) {
                    *sample = sample.map(|value| value * amplitude);
                }
                continue;
            }
            for (n, sample) in rest.iter_mut().enumerate() {
                let cents = cutoffs[n];
                if cents != self.state.cutoff_modulation.0 {
                    self.state.cutoff_modulation = (cents, 2.0_f64.powf(cents / 1200.0));
                }
                *sample = self.state.filter.process_sample(
                    sample.map(|value| value * amplitudes[n]),
                    self.state.filter_type,
                    &FilterParameters {
                        cutoff_hz: (self.state.oscillator_inputs[0].frequency_hz
                            * self.state.cutoff_ratio
                            * self.state.cutoff_modulation.1)
                            .min(f64::MAX),
                        resonance_q: self.state.resonance_q,
                    },
                    sample_rate,
                );
            }
        }
    }

    #[inline]
    fn generate_one_sample<const PREPARED: bool>(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
    ) -> [f64; 2] {
        if sample_rate != self.state.sample_rate {
            if !(sample_rate.is_finite() & (sample_rate > 0.0)) {
                return [0.0; 2];
            }
            self.state.oscillators_prepared = 0;
            self.state.sample_rate = sample_rate;
            if self.state.glide_remaining_ms > 0.0 {
                self.update_glide_ratio();
            }
            self.state.envelope.set_input(
                &self.state.envelope_parameters,
                sample_rate,
                self.state.amplitude,
            );
            self.state.cutoff_envelope.set_input(
                &self.state.cutoff_envelope_parameters,
                sample_rate,
                1.0,
            );
        }
        let amplitude = self.state.envelope.generate_sample();
        let level = self.state.cutoff_envelope.generate_sample();
        let cents = level * self.state.velocity_cutoff_amount;
        // バイパス中もEnvelopeは進め、有効化したサンプルで現在値の倍率を求める。
        if !matches!(self.state.filter_type, FilterType::None)
            && cents != self.state.cutoff_modulation.0
        {
            self.state.cutoff_modulation = (cents, 2.0_f64.powf(cents / 1200.0));
        }
        let active = self.state.envelope.is_active();
        let mono = [*mono_gains; 7];
        let mut sample = [0.0; 2];
        if active {
            let prepared = self.state.oscillators_prepared;
            let depths = [0.0, self.state.phase_depths[0], self.state.phase_depths[1]];
            let mut modulation = 0.0;
            // 同じサンプル時刻の生波形をOsc3→Osc2→Osc1へ渡す。
            for i in (0..3).rev() {
                self.state.oscillator_inputs[i].active = true;
                let input = &mut self.state.oscillator_inputs[i];
                input.phase_modulation = modulation;
                input.phase_modulated = i < 2 && self.state.phase_depths[i] != 0.0;
                // SAFETY: 定常設定・周波数・レートは親とVoiceで検証済み。
                // 設定・ノート変更は各入口で対応する準備済みビットを失効させる。
                // 変調入力は有限の生波形と0〜2周期の深度から生成する。
                if !PREPARED && prepared & (1 << i) == 0 {
                    unsafe { self.state.oscillators[i].prepare_sample(input, sample_rate) };
                }
                if self.state.oscillator_gains[i] == 0.0 && depths[i] == 0.0 {
                    // SAFETY: 同じ定常入力を上記の入口か前サンプルで反映済み。
                    unsafe {
                        self.state.oscillators[i].generate_prepared_sample::<false>(input);
                    }
                    modulation = 0.0;
                    continue;
                }
                // SAFETY: 上記と同じ準備済み入力。再発音はNote Onを通る。
                let current =
                    unsafe { self.state.oscillators[i].generate_prepared_sample::<true>(input) };
                modulation = current.values[0] * depths[i];
                if self.state.oscillator_gains[i] != 0.0 {
                    // 無音の変調源はrawだけを使い、音声用の定位・加算を省く。
                    let audio = volume::process_sample(
                        &current.values[..current.count],
                        if i == 0 { gains } else { &mono },
                        self.state.oscillator_gains[i],
                    );
                    sample[0] += audio[0];
                    sample[1] += audio[1];
                }
            }
            if !PREPARED {
                self.state.oscillators_prepared = 0b111;
            }
        } else {
            // 再発音は必ずNote Onを通り、各Oscillatorの世代変更で履歴を更新する。
            for input in &mut self.state.oscillator_inputs {
                input.active = false;
            }
        }
        let sample = sample.map(|value| value * amplitude);
        let output = self.state.filter.process_sample(
            sample,
            self.state.filter_type,
            &FilterParameters {
                cutoff_hz: (self.state.oscillator_inputs[0].frequency_hz
                    * self.state.cutoff_ratio
                    * self.state.cutoff_modulation.1)
                    .min(f64::MAX),
                resonance_q: self.state.resonance_q,
            },
            sample_rate,
        );
        self.state.envelope.update_sample();
        self.state.cutoff_envelope.update_sample();
        if !PREPARED && active {
            self.update_pitch_sample();
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portamento_tracks_cents_retargets_and_starts_silent_notes_immediately() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let mut voice = Voice::new();
            voice.set_parameters(
                Waveform::Sine,
                EnvelopeParameters {
                    attack_ms: 0.0,
                    hold_ms: 0.0,
                    decay_ms: 0.0,
                    sustain: 1.0,
                    release_ms: 1.0,
                },
                1,
                0.0,
                PhaseMode::Keep,
            );
            voice.set_portamento(10.0);
            voice.set_pitch_ratio(2.0);
            voice.set_oscillator(1, Waveform::Sine, 0.5, 1.0);
            voice.set_oscillator(2, Waveform::Sine, 4.0, 1.0);
            let note = Note {
                id: 1,
                channel: 0,
                pitch: 69,
            };
            let next = Note {
                id: 2,
                pitch: 81,
                ..note
            };
            assert!(voice.note_on(note, 0.0, 1.0));
            assert_eq!(voice.state.note_hz, 440.0);
            voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
            assert!(voice.note_on(next, 0.0, 1.0));
            let count = (rate * 0.01).round() as usize;
            for i in 0..=count {
                let expected = 440.0 * 2.0_f64.powf(i as f64 / count as f64);
                assert!((voice.state.note_hz / expected - 1.0).abs() < 1e-10);
                for (input, ratio) in voice.state.oscillator_inputs.iter().zip([2.0, 1.0, 8.0]) {
                    assert!((input.frequency_hz / (expected * ratio) - 1.0).abs() < 1e-10);
                }
                voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
            }
            assert_eq!(voice.state.glide_remaining_ms, 0.0);
            voice.note_off(next);
            assert!(voice.note_on(note, 0.0, 1.0)); // release中からの移行。
            for _ in 0..count / 2 {
                voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
            }
            let current = voice.state.note_hz;
            assert!(voice.note_on(next, 0.0, 1.0));
            assert_eq!(voice.state.note_hz, current);
            voice.set_portamento(0.0);
            assert_eq!(voice.state.note_hz, 880.0);
            voice.set_portamento(10.0);
            voice.note_off(next);
            for _ in 0..count {
                voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
            }
            assert!(voice.note_on(note, 0.0, 1.0));
            assert_eq!(voice.state.note_hz, 440.0);
            assert_eq!(voice.state.glide_remaining_ms, 0.0);
            voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
            voice.stop();
            assert!(voice.note_on(next, 0.0, 1.0));
            assert_eq!(voice.state.note_hz, 880.0);
            assert_eq!(voice.state.glide_remaining_ms, 0.0);
        }
    }

    #[test]
    fn portamento_series_matches_samples_with_unison_modulation_and_filter() {
        for rate in [44100.0, 96000.0] {
            for modulation in [0.0, 0.5] {
                let mut voice = Voice::new();
                voice.set_parameters(
                    Waveform::Sawtooth,
                    DEFAULT_ENVELOPE,
                    7,
                    25.0,
                    PhaseMode::Zero,
                );
                voice.set_filter(FilterType::LowPass2, 1200.0, 1.0);
                voice.set_phase_modulation([modulation; 2]);
                voice.set_portamento(1.0);
                let note = Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                };
                assert!(voice.note_on(note, 0.0, 1.0));
                voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
                assert!(voice.note_on(
                    Note {
                        id: 2,
                        pitch: 81,
                        ..note
                    },
                    0.0,
                    1.0
                ));
                let mut reference = voice.clone();
                for length in [1, 3, 32, 65, 512] {
                    let mut output = vec![[0.0; 2]; length];
                    voice.generate_samples(rate, &[[1.0; 2]; 7], &[1.0; 2], &mut output);
                    let mut error = 0.0;
                    let mut energy = 0.0;
                    for actual in output {
                        let expected = reference.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
                        error += (actual[0] - expected[0]).powi(2);
                        energy += expected[0].powi(2);
                    }
                    assert!(error <= energy * 1e-8, "{error}/{energy}");
                }
            }
        }
    }

    #[test]
    fn velocity_response_matches_scaled_inputs_in_sample_and_series_paths() {
        let shape = EnvelopeParameters {
            attack_ms: 0.0,
            hold_ms: 0.0,
            decay_ms: 0.0,
            sustain: 1.0,
            release_ms: 1.0,
        };
        let note = Note {
            id: 1,
            channel: 0,
            pitch: 69,
        };
        for rate in [44100.0, 48000.0, 96000.0] {
            for velocity in [0.0_f32, 0.25, 1.0] {
                for amp in [0.0, 0.5, 1.0] {
                    for filter in [0.0, 0.5, 1.0] {
                        for amount in [-2400.0, 2400.0] {
                            for depth in [0.0, 0.3] {
                                let mut actual = Voice::new();
                                let mut reference = Voice::new();
                                let amp_gain = (1.0 - amp) + amp * f64::from(velocity);
                                let cutoff_gain = (1.0 - filter) + filter * f64::from(velocity);
                                for voice in [&mut actual, &mut reference] {
                                    voice.set_parameters(
                                        Waveform::Sine,
                                        shape,
                                        1,
                                        0.0,
                                        PhaseMode::Zero,
                                    );
                                    voice.set_filter(FilterType::LowPass2, 0.0, 1.0);
                                    voice.set_phase_modulation([depth, depth]);
                                }
                                actual.set_cutoff_envelope(shape, amount);
                                actual.set_velocity_sensitivity([amp, filter]);
                                reference.set_cutoff_envelope(shape, amount * cutoff_gain);
                                assert!(actual.note_on(note, 0.0, velocity));
                                assert!(reference.note_on(note, 0.0, amp_gain as f32));
                                let mut single = actual.clone();
                                for block in 0..4 {
                                    if block == 2 {
                                        for voice in [&mut actual, &mut single, &mut reference] {
                                            voice.note_off(note);
                                        }
                                    }
                                    let mut output = [[0.0; 2]; 65];
                                    actual.generate_samples(
                                        rate,
                                        &[[1.0; 2]; 7],
                                        &[1.0; 2],
                                        &mut output,
                                    );
                                    for sample in output {
                                        let expected = reference.generate_sample(
                                            rate,
                                            &[[1.0; 2]; 7],
                                            &[1.0; 2],
                                        );
                                        let one =
                                            single.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
                                        for ch in 0..2 {
                                            assert!((sample[ch] - expected[ch]).abs() < 1e-12);
                                            assert!((one[ch] - expected[ch]).abs() < 1e-12);
                                        }
                                        if amp == 1.0 && velocity == 0.0 {
                                            assert_eq!(sample, [0.0; 2]);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sensitivity_updates_keep_attack_time_and_retrigger_from_current_level() {
        let shape = EnvelopeParameters {
            attack_ms: 4.0,
            hold_ms: 0.0,
            decay_ms: 0.0,
            sustain: 1.0,
            release_ms: 4.0,
        };
        let note = Note {
            id: 1,
            channel: 0,
            pitch: 69,
        };
        let mut voice = Voice::new();
        voice.set_parameters(Waveform::Sine, shape, 1, 0.0, PhaseMode::Zero);
        voice.set_cutoff_envelope(shape, 2400.0);
        voice.note_on(note, 0.0, 0.0);
        voice.generate_sample(1000.0, &[[1.0; 2]; 7], &[1.0; 2]);
        voice.set_velocity_sensitivity([0.0, 1.0]);
        // 1 ms経過したattackを継続する。フィルタAmountだけはゼロになる。
        assert_eq!(voice.state.envelope.generate_sample(), 0.125);
        assert_eq!(voice.state.velocity_cutoff_amount, 0.0);
        for _ in 1..4 {
            voice.generate_sample(1000.0, &[[1.0; 2]; 7], &[1.0; 2]);
        }
        voice.set_velocity_sensitivity([1.0, 0.5]);
        voice.note_on(note, 0.0, 0.5);
        // 現在値0.5から新しい目標0.25へ、attack全4 msを使う。
        for value in [0.5, 0.4375, 0.375, 0.3125, 0.25] {
            assert_eq!(voice.state.envelope.generate_sample(), value);
            voice.generate_sample(1000.0, &[[1.0; 2]; 7], &[1.0; 2]);
        }
        assert_eq!(voice.state.velocity_cutoff_amount, 1800.0);
        voice.note_off(note);
        voice.set_cutoff_envelope(shape, -4800.0);
        assert_eq!(voice.state.velocity_cutoff_amount, -3600.0);
        for _ in 0..5 {
            voice.generate_sample(1000.0, &[[1.0; 2]; 7], &[1.0; 2]);
        }
        assert!(!voice.state.envelope.is_active());
    }

    fn audio_values(oscillator: &mut Oscillator, input: &OscillatorInput, rate: f64) -> [f64; 7] {
        oscillator.generate_sample(input, rate).values
    }

    #[test]
    fn invalid_series_rate_preserves_voice_state() {
        let mut reference = Voice::new();
        let note = Note {
            id: 1,
            channel: 0,
            pitch: 69,
        };
        reference.set_phase_modulation([0.5, 0.3]);
        reference.note_on(note, 0.0, 1.0);
        for _ in 0..64 {
            reference.generate_sample(48_000.0, &[[1.0; 2]; 7], &[1.0; 2]);
        }
        // 独立した初期計算の丸め差を除き、同じ状態から不正入力の影響だけを比較する。
        let mut series = reference.clone();
        for rate in [0.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let mut output = [[7.0; 2]; 33];
            series.generate_samples(rate, &[[1.0; 2]; 7], &[1.0; 2], &mut output);
            assert_eq!(output, [[0.0; 2]; 33]);
        }
        for _ in 0..128 {
            assert_eq!(
                series.generate_sample(48_000.0, &[[1.0; 2]; 7], &[1.0; 2]),
                reference.generate_sample(48_000.0, &[[1.0; 2]; 7], &[1.0; 2]),
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 全波形・深度のFourier照合は通常のCPUテストで行う。
    fn phase_modulation_chain_matches_same_sample_raw_waveforms() {
        for rate in [44100.0, 48000.0, 96000.0] {
            for wave in [
                Waveform::Sine,
                Waveform::Triangle,
                Waveform::Square,
                Waveform::Sawtooth,
            ] {
                for depths in [
                    [0.0, 0.0],
                    [0.0, 1.0],
                    [1.0, 0.0],
                    [0.37, 0.61],
                    [1.0, 1.0],
                    [1.5, 2.0],
                    [2.0, 2.0],
                ] {
                    let mut voice = Voice::new();
                    let shape = EnvelopeParameters {
                        attack_ms: 0.0,
                        hold_ms: 0.0,
                        decay_ms: 0.0,
                        sustain: 1.0,
                        release_ms: 0.0,
                    };
                    voice.set_parameters(Waveform::Sine, shape, 3, 25.0, PhaseMode::Zero);
                    voice.set_filter(FilterType::None, 4800.0, 1.0);
                    voice.set_oscillator(1, wave, 2.0, 0.0);
                    voice.set_oscillator(2, wave, 0.5, 0.0);
                    voice.set_phase_modulation(depths);
                    let note = Note {
                        id: 1,
                        channel: 0,
                        pitch: 69,
                    };
                    voice.note_on(note, 0.0, 1.0);
                    let mut carrier = [0.0_f64; 3];
                    let mut modulators = [0.0_f64; 2];
                    let mut histories = [[0.0; 2]; 4];
                    for n in 0..256 {
                        if n == 127 {
                            // Phase=Zeroの再トリガーを全段へ適用する。
                            voice.note_on(note, 0.0, 1.0);
                            carrier.fill(0.0);
                            modulators.fill(0.0);
                        }
                        let m3 = crate::oscillator::reference::raw_sample(
                            wave,
                            modulators[1].rem_euclid(1.0),
                            220.0 / rate,
                        );
                        // Reference uses analytic Fourier antiderivatives, not production tables.
                        let mut smoothed = |slot: usize, wave, q: f64, inc: f64, enabled: bool| {
                            let [b, c] = if n == 0 || n == 127 {
                                [q - inc, q - 2.0 * inc]
                            } else {
                                histories[slot]
                            };
                            histories[slot] = [q, b];
                            if enabled {
                                crate::oscillator::reference::antialiased_sample(
                                    wave,
                                    [q, b, c],
                                    inc,
                                )
                            } else {
                                crate::oscillator::reference::raw_sample(
                                    wave,
                                    q.rem_euclid(1.0),
                                    inc,
                                )
                            }
                        };
                        let m2 = smoothed(
                            3,
                            wave,
                            modulators[0] + depths[1] * m3,
                            880.0 / rate,
                            depths[1] != 0.0,
                        );
                        let expected = carrier
                            .iter()
                            .zip([-25.0_f64, 0.0, 25.0])
                            .enumerate()
                            .map(|(i, (p, cents))| {
                                smoothed(
                                    i,
                                    Waveform::Sine,
                                    p + depths[0] * m2,
                                    440.0 * 2.0_f64.powf(cents / 1200.0) / rate,
                                    depths[0] != 0.0,
                                )
                            })
                            .sum::<f64>()
                            * 0.5
                            * std::f64::consts::SQRT_2
                            / 3.0_f64.sqrt();
                        let actual = voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
                        // 2段の位相変調では正弦波LUTの誤差も最大4π倍ずつ増幅される。
                        let tolerance = 7e-5;
                        assert!(
                            (actual[0] - expected).abs() < tolerance,
                            "rate={rate}, wave={wave:?}, depths={depths:?}, n={n}, actual={actual:?}, expected={expected}"
                        );
                        assert_eq!(actual[0], actual[1]);
                        for (p, cents) in carrier.iter_mut().zip([-25.0_f64, 0.0, 25.0]) {
                            *p += 440.0 * 2.0_f64.powf(cents / 1200.0) / rate;
                        }
                        modulators[0] += 880.0 / rate;
                        modulators[1] += 220.0 / rate;
                        assert_eq!(
                            voice.state.oscillator_inputs.map(|i| i.frequency_hz),
                            [440.0, 880.0, 220.0]
                        );
                    }
                    voice.note_off(note);
                    assert_eq!(
                        voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]),
                        [0.0; 2]
                    );
                }
            }
        }
    }

    #[test]
    fn square_modulator_retains_amplitude_above_one() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let mut voice = Voice::new();
            let shape = EnvelopeParameters {
                attack_ms: 0.0,
                hold_ms: 0.0,
                decay_ms: 0.0,
                sustain: 1.0,
                release_ms: 0.0,
            };
            voice.set_parameters(Waveform::Sine, shape, 1, 0.0, PhaseMode::Zero);
            voice.set_filter(FilterType::None, 4800.0, 1.0);
            voice.set_oscillator(1, Waveform::Square, 4.0, 0.0);
            voice.set_phase_modulation([2.0, 0.0]);
            voice.note_on(
                Note {
                    id: 1,
                    channel: 0,
                    pitch: 108,
                },
                0.0,
                1.0,
            );
            let frequency = voice.state.oscillator_inputs[0].frequency_hz;
            let (mut carrier, mut modulator, mut max_offset) = (0.0_f64, 0.0_f64, 0.0_f64);
            let mut history = [-frequency / rate, -2.0 * frequency / rate];
            for _ in 0..256 {
                // この音程の矩形波は全レートで基音だけが残り、振幅は4/π。
                let offset =
                    2.0 * (4.0 / std::f64::consts::PI) * (std::f64::consts::TAU * modulator).sin();
                let q = carrier + offset;
                let expected = 0.5
                    * std::f64::consts::SQRT_2
                    * crate::oscillator::reference::antialiased_sample(
                        Waveform::Sine,
                        [q, history[0], history[1]],
                        frequency / rate,
                    );
                history = [q, history[0]];
                let actual = voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
                assert!((actual[0] - expected).abs() < 2e-6);
                assert_eq!(actual[0], actual[1]);
                max_offset =
                    max_offset.max(voice.state.oscillator_inputs[0].phase_modulation.abs());
                carrier += frequency / rate;
                modulator = (modulator + frequency * 4.0 / rate).fract();
            }
            assert!(max_offset > 2.5);
        }
    }

    #[test]
    fn three_oscillators_share_envelope_and_phase_and_sum_before_filter() {
        for rate in [44100.0, 48000.0, 96000.0] {
            for tune in [-2400.0, 0.0, 2400.0] {
                let mut voice = Voice::new();
                let shape = EnvelopeParameters {
                    attack_ms: 1.0,
                    hold_ms: 0.0,
                    decay_ms: 1.0,
                    sustain: 0.6,
                    release_ms: 2.0,
                };
                voice.set_parameters(Waveform::Sine, shape, 1, 0.0, PhaseMode::Zero);
                let ratio = 2.0_f64.powf(tune / 1200.0);
                for (i, r) in [1.0, ratio, 1.0 / ratio].into_iter().enumerate() {
                    voice.set_oscillator(i, Waveform::Sine, r, 1.0);
                }
                voice.set_filter(FilterType::LowPass2, 1200.0, 1.0);
                let note = Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                };
                assert!(voice.note_on(note, 0.0, 1.0));
                let mut filter = Filter::new();
                let mut phase = [0.0_f64; 3];
                for n in 0..600 {
                    if n == 200 {
                        voice.note_off(note);
                    }
                    let level = crate::envelope::value(
                        &shape,
                        n as f64 * 1000.0 / rate,
                        (n >= 200).then_some(200.0 * 1000.0 / rate),
                        0.0,
                        0.5,
                    );
                    let sum = phase
                        .iter()
                        .map(|p| std::f64::consts::SQRT_2 * (2.0 * std::f64::consts::PI * p).sin())
                        .sum::<f64>()
                        * level;
                    let expected = filter.process_sample(
                        [sum; 2],
                        FilterType::LowPass2,
                        &FilterParameters {
                            cutoff_hz: 880.0,
                            resonance_q: 1.0,
                        },
                        rate,
                    );
                    let actual = voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
                    assert!(
                        (actual[0] - expected[0]).abs() < 6e-7,
                        "rate={rate}, tune={tune}, n={n}"
                    );
                    assert_eq!(actual[0], actual[1]);
                    for (p, r) in phase.iter_mut().zip([1.0, ratio, 1.0 / ratio]) {
                        *p = (*p + 440.0 * r / rate).rem_euclid(1.0);
                    }
                }
                assert_eq!(
                    voice.state.oscillator_inputs.map(|i| i.phase_mode),
                    [PhaseMode::Zero; 3]
                );
            }
        }
    }

    #[test]
    fn muted_oscillators_keep_phase_and_tune_changes_do_not_restart_it() {
        let mut muted = Voice::new();
        let mut reference = Voice::new();
        let shape = EnvelopeParameters {
            attack_ms: 0.0,
            hold_ms: 0.0,
            decay_ms: 0.0,
            sustain: 1.0,
            release_ms: 0.0,
        };
        let note = Note {
            id: 1,
            channel: 0,
            pitch: 69,
        };
        for voice in [&mut muted, &mut reference] {
            voice.set_filter(FilterType::None, 4800.0, 1.0);
            voice.set_parameters(Waveform::Sine, shape, 7, 25.0, PhaseMode::Keep);
            voice.set_oscillator(0, Waveform::Sine, 1.0, 0.0);
            voice.set_oscillator(1, Waveform::Sine, 2.0, 1.0);
            voice.set_oscillator(2, Waveform::Triangle, 0.5, 0.5);
            voice.note_on(note, 0.0, 1.0);
        }
        muted.set_oscillator(1, Waveform::Sine, 2.0, 0.0);
        muted.set_oscillator(2, Waveform::Triangle, 0.5, 0.0);
        for n in 0..128 {
            if n == 64 {
                muted.set_oscillator(1, Waveform::Sine, 2.0, 1.0);
                muted.set_oscillator(2, Waveform::Triangle, 0.5, 0.5);
            }
            if n == 96 {
                for voice in [&mut muted, &mut reference] {
                    voice.set_oscillator(1, Waveform::Sine, 4.0, 1.0);
                }
            }
            let actual =
                muted.generate_sample(48000.0, &[[1.0; 2]; 7], &[0.0, std::f64::consts::SQRT_2]);
            let expected = reference.generate_sample(
                48000.0,
                &[[1.0; 2]; 7],
                &[0.0, std::f64::consts::SQRT_2],
            );
            if n < 64 {
                assert_eq!(actual, [0.0; 2]);
            } else {
                assert_eq!(actual, expected);
            }
            assert_eq!(actual[0], 0.0); // Osc2/3はOsc1のWidthではなく共通Panに従う。
        }
        assert_eq!(muted.state.oscillator_inputs[1].unison_voices, 1);
        assert_eq!(muted.state.oscillator_inputs[2].detune_cents, 0.0);
        muted.stop();
        assert_eq!(
            muted.generate_sample(48000.0, &[[1.0; 2]; 7], &[1.0; 2]),
            [0.0; 2]
        );
    }

    #[test]
    fn enabling_filter_uses_the_envelope_advanced_during_bypass() {
        let mut bypassed = Voice::new();
        let mut reference = Voice::new();
        let note = Note {
            id: 1,
            channel: 0,
            pitch: 69,
        };
        bypassed.set_filter(FilterType::None, 1200.0, 2.0);
        reference.set_filter(FilterType::LowPass2, 1200.0, 2.0);
        for voice in [&mut bypassed, &mut reference] {
            voice.set_cutoff_envelope(DEFAULT_ENVELOPE, 2400.0);
            voice.note_on(note, 0.0, 1.0);
        }
        for n in 0..96 {
            if n == 32 {
                bypassed.note_off(note);
                reference.note_off(note);
            }
            if n == 48 {
                bypassed.note_on(note, 0.0, 1.0);
                reference.note_on(note, 0.0, 1.0);
            }
            bypassed.generate_sample(48000.0, &[[1.0; 2]; 7], &[1.0; 2]);
            reference.generate_sample(48000.0, &[[1.0; 2]; 7], &[1.0; 2]);
        }
        // 有効中に進めた参照側も、バイパス解除時と同じ無履歴から比較する。
        reference.state.filter.reset();
        bypassed.set_filter(FilterType::LowPass2, 1200.0, 2.0);
        for _ in 0..128 {
            assert_eq!(
                bypassed.generate_sample(48000.0, &[[1.0; 2]; 7], &[1.0; 2])[0],
                reference.generate_sample(48000.0, &[[1.0; 2]; 7], &[1.0; 2])[0]
            );
        }
    }

    #[test]
    fn cutoff_envelope_modulates_cents_and_retriggers_from_its_current_value() {
        let shape = EnvelopeParameters {
            attack_ms: 4.0,
            hold_ms: 2.0,
            decay_ms: 4.0,
            sustain: 0.25,
            release_ms: 4.0,
        };
        let levels = [
            0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 0.8125, 0.625, 0.46875, 0.3125, 0.484375, 0.65625,
            0.828125, 1.0, 1.0, 1.0, 0.8125, 0.625, 0.4375, 0.328125, 0.21875, 0.109375, 0.0,
        ];
        for kind in parameters::FILTER_TYPES {
            for amount in [-4800.0, 0.0, 4800.0] {
                let mut voice = Voice::new();
                voice.state.envelope_parameters = EnvelopeParameters {
                    attack_ms: 0.0,
                    hold_ms: 0.0,
                    decay_ms: 0.0,
                    sustain: 1.0,
                    release_ms: 0.0,
                };
                voice.set_filter(kind, -1200.0, 1.0);
                voice.set_cutoff_envelope(shape, amount);
                let mut oscillator = Oscillator::new();
                let mut filter = Filter::new();
                for (n, level) in levels.into_iter().enumerate() {
                    let note = Note {
                        id: 1,
                        channel: 0,
                        pitch: 57,
                    };
                    if n == 0 || n == 10 {
                        assert!(voice.note_on(note, 0.0, if n == 0 { 1.0 } else { 0.5 }));
                    }
                    if n == 8 || n == 19 {
                        voice.note_off(note);
                    }
                    let mut input = voice.state.oscillator_inputs[0];
                    input.active = voice.state.gate;
                    let expected = filter.process_sample(
                        volume::process_sample(
                            &audio_values(&mut oscillator, &input, 1000.0)
                                [..input.rendered_voices()],
                            &[[1.0; 2]; 7],
                            voice.state.amplitude
                                * volume::gain(input.waveform, input.rendered_voices(), 1.0),
                        ),
                        kind,
                        &FilterParameters {
                            cutoff_hz: 220.0 * 2.0_f64.powf((-1200.0 + amount * level) / 1200.0),
                            resonance_q: 1.0,
                        },
                        1000.0,
                    )[0];
                    assert!(
                        (voice.generate_sample(1000.0, &[[1.0; 2]; 7], &[1.0; 2])[0] - expected)
                            .abs()
                            < 1e-12,
                        "sample={n}, amount={amount}, type={kind:?}"
                    );
                }
                voice.stop();
                assert_eq!(
                    voice.generate_sample(1000.0, &[[1.0; 2]; 7], &[1.0; 2])[0],
                    0.0
                );
                assert_eq!(voice.state.cutoff_modulation.0, 0.0);
            }
        }
    }

    #[test]
    fn cutoff_tracks_note_tuning_and_filters_after_unison() {
        for filter_type in parameters::FILTER_TYPES {
            for cents in [-2400.0, 1200.0, 9600.0] {
                let mut voice = Voice::new();
                voice.set_parameters(
                    Waveform::Sawtooth,
                    EnvelopeParameters {
                        attack_ms: 0.0,
                        hold_ms: 0.0,
                        decay_ms: 0.0,
                        sustain: 1.0,
                        release_ms: 0.0,
                    },
                    3,
                    37.0,
                    PhaseMode::Keep,
                );
                voice.set_filter(filter_type, cents, 2.0);
                assert!(voice.note_on(
                    Note {
                        id: 1,
                        channel: 0,
                        pitch: 69
                    },
                    100.0,
                    1.0
                ));
                let mut input = voice.state.oscillator_inputs[0];
                input.active = true;
                let mut oscillator = Oscillator::new();
                let mut reference = Filter::new();
                let settings = FilterParameters {
                    cutoff_hz: 440.0 * 2.0_f64.powf(100.0 / 1200.0) * 2.0_f64.powf(cents / 1200.0),
                    resonance_q: 2.0,
                };
                for _ in 0..64 {
                    let expected = reference.process_sample(
                        volume::process_sample(
                            &audio_values(&mut oscillator, &input, 48_000.0)
                                [..input.rendered_voices()],
                            &[[1.0; 2]; 7],
                            voice.state.amplitude
                                * volume::gain(input.waveform, input.rendered_voices(), 1.0),
                        ),
                        filter_type,
                        &settings,
                        48_000.0,
                    )[0];
                    assert!(
                        (voice.generate_sample(48_000.0, &[[1.0; 2]; 7], &[1.0; 2])[0] - expected)
                            .abs()
                            < 1e-12
                    );
                }
                voice.note_off(Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                });
                input.active = false;
                for _ in 0..16 {
                    let expected = reference.process_sample(
                        volume::process_sample(
                            &audio_values(&mut oscillator, &input, 48_000.0)
                                [..input.rendered_voices()],
                            &[[1.0; 2]; 7],
                            voice.state.amplitude
                                * volume::gain(input.waveform, input.rendered_voices(), 1.0),
                        ),
                        filter_type,
                        &settings,
                        48_000.0,
                    )[0];
                    assert!(
                        (voice.generate_sample(48_000.0, &[[1.0; 2]; 7], &[1.0; 2])[0] - expected)
                            .abs()
                            < 1e-12
                    );
                }
                voice.stop();
                assert_eq!(
                    voice.generate_sample(48_000.0, &[[1.0; 2]; 7], &[1.0; 2])[0],
                    0.0
                );
            }
        }
    }
}

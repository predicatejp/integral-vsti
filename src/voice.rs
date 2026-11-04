// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::distortion::{Distortion, DistortionParameters};
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
pub struct VoiceState {
    /// Osc1・Osc2・Osc3の定常入力。Osc1だけがユニゾン設定を持つ。
    pub(super) oscillator_inputs: [OscillatorInput; 3],
    /// 発振器ごとに独立した位相・乱数の状態。
    oscillators: [Oscillator; 3],
    /// Volの線形倍率。−∞ dBでは0。
    oscillator_volumes: [f64; 3],
    /// 波形・ユニゾン補正とVolを合わせた倍率。設定変更時に計算する。
    oscillator_gains: [f64; 3],
    /// Osc2→Osc1、Osc3→Osc2の位相変調量（0〜2周期）。
    phase_depths: [f64; 2],
    /// ノート基音に掛けるOsc2/3のTune倍率。Osc1は1。
    tune_ratios: [f64; 3],
    /// ノートを保持している間true。Note Offでは共通AHDSRのreleaseへ移る。
    pub(super) gate: bool,
    /// Note Onの世代。3発振器へ同じ番号を渡す。
    note_generation: u64,
    /// 両Envelopeへ反映済みの有効なレート。生成時に変わった場合だけ再検証する。
    sample_rate: f64,
    /// ベロシティを反映した共通AHDSRの目標実効振幅。
    amplitude: f64,
    /// 発振器の加算出力へ適用する音量AHDSRの設定。
    pub(super) envelope_parameters: EnvelopeParameters,
    /// 3発振器に共通の音量AHDSRと時間履歴。
    envelope: Envelope,
    /// Note Offの照合対象。発振器へノート情報を渡さない。
    note: Option<Note>,
    /// クリッパーへ渡すフィルタ。ノートオフ後も残響を処理する。
    filter: Filter,
    /// フィルタ後のクリッパーと、左右独立のADAA履歴。
    distortion: Distortion,
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
    /// 直近の変調量centと周波数倍率。同じ値では指数計算を省く。
    cutoff_modulation: (f64, f64),
}

/// Voiceの処理を担当し、専用のStateを所有するコンポーネント。
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
                oscillator_volumes: [1.0, 0.0, 0.0],
                oscillator_gains: [std::f64::consts::SQRT_2, 0.0, 0.0],
                phase_depths: [0.0; 2],
                tune_ratios: [1.0; 3],
                gate: false,
                note_generation: 0,
                sample_rate: MIN_SAMPLE_RATE,
                amplitude: 0.0,
                envelope_parameters: DEFAULT_ENVELOPE,
                envelope,
                note: None,
                filter: Filter::new(),
                distortion: Distortion::new(),
                filter_type: parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize],
                cutoff_ratio: 2.0_f64.powf(parameters::to_plain(10, values[10]) / 1200.0),
                resonance_q: parameters::to_plain(11, values[11]),
                cutoff_envelope_parameters: DEFAULT_ENVELOPE,
                cutoff_envelope,
                cutoff_amount_cents: 0.0,
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
        self.state.note_generation = self.state.note_generation.wrapping_add(1);
        for (input, ratio) in self
            .state
            .oscillator_inputs
            .iter_mut()
            .zip(self.state.tune_ratios)
        {
            input.frequency_hz = (frequency_hz * ratio).min(f64::MAX);
            input.note_generation = self.state.note_generation;
        }
        self.state.amplitude = BASE_RMS_AMPLITUDE * f64::from(velocity);
        self.state.envelope.set_input(
            &self.state.envelope_parameters,
            self.state.sample_rate,
            self.state.amplitude,
        );
        self.state.envelope.note_on();
        self.state.cutoff_envelope.note_on();
        self.state.gate = true;
        self.state.note = Some(note);
        true
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
        // releaseとは区別し、次サンプルで即時消音する。位相は保持する。
        self.state.gate = false;
        self.state.envelope.stop();
        self.state.cutoff_envelope.stop();
        self.state.note = None;
        self.state.filter.reset();
        self.reset_distortion();
        for input in &mut self.state.oscillator_inputs {
            input.active = false;
        }
    }

    pub fn reset_distortion(&mut self) {
        self.state.distortion.reset();
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
    }

    pub fn set_parameters(
        &mut self,
        waveform: Waveform,
        envelope: EnvelopeParameters,
        unison: u32,
        detune: f64,
        phase_mode: PhaseMode,
    ) {
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
        let input = &mut self.state.oscillator_inputs[index];
        input.waveform = waveform;
        input.frequency_hz = (base_hz * tune_ratio).min(f64::MAX);
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
        self.state.phase_depths = depths;
    }

    /// フィルタ後のDryと、2階ADAAでクリップしたWetを返す。
    #[inline(never)]
    pub fn generate_distorted_sample(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
        parameters: &DistortionParameters,
    ) -> ([f64; 2], [f64; 2]) {
        let input = self.generate_sample(sample_rate, gains, mono_gains);
        let wet = self.state.distortion.process_sample(input, parameters);
        (input, wet)
    }

    /// Voiceの3発振器・音量AHDSR・SVFまでをホストレートで処理する。
    pub fn generate_sample(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
    ) -> [f64; 2] {
        if sample_rate != self.state.sample_rate {
            if !(sample_rate.is_finite() & (sample_rate > 0.0)) {
                return [0.0; 2];
            }
            self.state.sample_rate = sample_rate;
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
        let cents = level * self.state.cutoff_amount_cents;
        // バイパス中もEnvelopeは進め、有効化したサンプルで現在値の倍率を求める。
        if !matches!(self.state.filter_type, FilterType::None)
            && cents != self.state.cutoff_modulation.0
        {
            self.state.cutoff_modulation = (cents, 2.0_f64.powf(cents / 1200.0));
        }
        let active = self.state.envelope.is_active();
        let mono = [*mono_gains; 7];
        let mut sample = [0.0; 2];
        let depths = [0.0, self.state.phase_depths[0], self.state.phase_depths[1]];
        let mut modulation = 0.0;
        // 同じサンプル時刻の生波形をOsc3→Osc2→Osc1へ渡す。
        for i in (0..3).rev() {
            self.state.oscillator_inputs[i].active = active;
            let input = &mut self.state.oscillator_inputs[i];
            input.phase_modulation = modulation;
            input.phase_modulated = i < 2 && self.state.phase_depths[i] != 0.0;
            if self.state.oscillator_gains[i] == 0.0 && depths[i] == 0.0 {
                // SAFETY: 定常設定は親、ノート周波数とレートはVoiceで検証済み。
                // 変調入力は有限の波形と0〜2周期の深度から生成し、i32範囲内に収まる。
                unsafe {
                    self.state.oscillators[i]
                        .generate_validated_sample::<false>(input, sample_rate);
                }
                modulation = 0.0;
                continue;
            }
            // SAFETY: 上記と同じ検証済みの定常設定・周波数・レート・変調入力を使う。
            let current = unsafe {
                self.state.oscillators[i].generate_validated_sample::<true>(input, sample_rate)
            };
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
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_values(oscillator: &mut Oscillator, input: &OscillatorInput, rate: f64) -> [f64; 7] {
        oscillator.generate_sample(input, rate).values
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

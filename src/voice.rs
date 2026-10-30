// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::envelope::{DEFAULT_ENVELOPE, Envelope, EnvelopeParameters};
use crate::filter::{Filter, FilterParameters, FilterType};
use crate::oscillator::{Oscillator, OscillatorInput, PanningGains, PhaseMode, Waveform};
use crate::parameters;

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
    /// 発振器ごとの音量倍率。−∞ dBでは0。
    oscillator_gains: [f64; 3],
    /// ノート基音に掛けるOsc2/3のTune倍率。Osc1は1。
    tune_ratios: [f64; 3],
    /// ノートを保持している間true。Note Offでは共通AHDSRのreleaseへ移る。
    pub(super) gate: bool,
    /// Note Onの世代。3発振器と両AHDSRへ同じ番号を渡す。
    note_generation: u64,
    /// 即時停止の世代。両AHDSRへ渡す。
    reset_generation: u64,
    /// ベロシティを反映した共通AHDSRの目標実効振幅。
    amplitude: f64,
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
        Self {
            state: VoiceState {
                oscillator_inputs: [OscillatorInput {
                    waveform: Waveform::Sine,
                    phase_mode: PhaseMode::Keep,
                    unison_voices: 1,
                    detune_cents: 0.0,
                    frequency_hz: 440.0,
                    amplitude: 1.0,
                    active: false,
                    note_generation: 0,
                }; 3],
                oscillators: std::array::from_fn(|_| Oscillator::new()),
                oscillator_gains: [1.0, 0.0, 0.0],
                tune_ratios: [1.0; 3],
                gate: false,
                note_generation: 0,
                reset_generation: 0,
                amplitude: 0.0,
                envelope_parameters: DEFAULT_ENVELOPE,
                envelope: Envelope::new(),
                note: None,
                filter: Filter::new(),
                filter_type: parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize],
                cutoff_ratio: 2.0_f64.powf(parameters::to_plain(10, values[10]) / 1200.0),
                resonance_q: parameters::to_plain(11, values[11]),
                cutoff_envelope_parameters: DEFAULT_ENVELOPE,
                cutoff_envelope: Envelope::new(),
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
            self.state.note = None;
        }
    }

    pub fn stop(&mut self) {
        // releaseとは区別し、次サンプルで即時消音する。位相は保持する。
        self.state.gate = false;
        self.state.reset_generation = self.state.reset_generation.wrapping_add(1);
        self.state.note = None;
        self.state.filter.reset();
    }

    pub fn set_filter(&mut self, filter_type: FilterType, cutoff_cents: f64, resonance_q: f64) {
        self.state.filter_type = filter_type;
        self.state.cutoff_ratio = 2.0_f64.powf(cutoff_cents / 1200.0);
        self.state.resonance_q = resonance_q;
    }

    pub fn set_cutoff_envelope(&mut self, parameters: EnvelopeParameters, amount_cents: f64) {
        self.state.cutoff_envelope_parameters = parameters;
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
        self.state.oscillator_inputs[0].unison_voices = unison;
        self.state.oscillator_inputs[0].detune_cents = detune;
        for input in &mut self.state.oscillator_inputs {
            input.phase_mode = phase_mode;
        }
    }

    /// 設定変更時だけ親が換算したTune倍率・音量を配送する。
    pub fn set_oscillator(&mut self, index: usize, waveform: Waveform, tune_ratio: f64, gain: f64) {
        self.state.oscillator_gains[index] = gain;
        self.state.tune_ratios[index] = tune_ratio;
        let base_hz = self.state.oscillator_inputs[0].frequency_hz;
        let input = &mut self.state.oscillator_inputs[index];
        input.waveform = waveform;
        input.frequency_hz = (base_hz * tune_ratio).min(f64::MAX);
    }

    /// Voice内のステレオ処理パイプライン。生成した信号をSynthesizerが加算する。
    pub fn generate_sample(
        &mut self,
        sample_rate: f64,
        gains: &PanningGains,
        mono_gains: &[f64; 2],
    ) -> [f64; 2] {
        if !(sample_rate.is_finite() & (sample_rate > 0.0)) {
            return [0.0; 2];
        }
        let amplitude = self.state.envelope.generate_sample(
            &self.state.envelope_parameters,
            sample_rate,
            self.state.gate,
            self.state.note_generation,
            self.state.reset_generation,
            self.state.amplitude,
        );
        let level = self.state.cutoff_envelope.generate_sample(
            &self.state.cutoff_envelope_parameters,
            sample_rate,
            self.state.gate,
            self.state.note_generation,
            self.state.reset_generation,
            1.0,
        );
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
        for i in 0..3 {
            let input = &mut self.state.oscillator_inputs[i];
            input.active = active;
            let current = self.state.oscillators[i].generate_sample(
                input,
                sample_rate,
                if i == 0 { gains } else { &mono },
            );
            for channel in 0..2 {
                sample[channel] += current[channel] * self.state.oscillator_gains[i];
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
        self.state.envelope.update_sample(
            &self.state.envelope_parameters,
            sample_rate,
            self.state.amplitude,
        );
        self.state.cutoff_envelope.update_sample(
            &self.state.cutoff_envelope_parameters,
            sample_rate,
            1.0,
        );
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                        *p = (*p + 440.0 * r / rate).fract();
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
                    input.amplitude = voice.state.amplitude;
                    let expected = filter.process_sample(
                        [oscillator.generate_sample(&input, 1000.0, &[[1.0; 2]; 7])[0]; 2],
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
                input.amplitude = 0.5;
                let mut oscillator = Oscillator::new();
                let mut reference = Filter::new();
                let settings = FilterParameters {
                    cutoff_hz: 440.0 * 2.0_f64.powf(100.0 / 1200.0) * 2.0_f64.powf(cents / 1200.0),
                    resonance_q: 2.0,
                };
                for _ in 0..64 {
                    let expected = reference.process_sample(
                        [oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7])[0]; 2],
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
                        [oscillator.generate_sample(&input, 48_000.0, &[[1.0; 2]; 7])[0]; 2],
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

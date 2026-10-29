// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::envelope::{DEFAULT_ENVELOPE, Envelope, EnvelopeParameters};
use crate::filter::{Filter, FilterParameters, FilterType};
use crate::oscillator::{Oscillator, OscillatorInput, PhaseMode, Waveform};
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
    /// このVoice専用の発振器1個への定常入力。ユニゾン設定を含む。
    pub(super) oscillator_input: OscillatorInput,
    /// このVoice専用の発振器の内部状態。ユニゾンの位相と音量履歴を保持する。
    oscillator: Oscillator,
    /// Note Offの照合対象。発音指示はoscillator_input.gateに反映し、発振器へノート情報を渡さない。
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
                oscillator_input: OscillatorInput {
                    waveform: Waveform::Sine,
                    phase_mode: PhaseMode::Keep,
                    unison_voices: 1,
                    detune_cents: 0.0,
                    frequency_hz: 440.0,
                    amplitude: 0.0,
                    gate: false,
                    envelope: DEFAULT_ENVELOPE,
                    note_generation: 0,
                    reset_generation: 0,
                },
                oscillator: Oscillator::new(),
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
        let input = OscillatorInput {
            waveform: self.state.oscillator_input.waveform,
            frequency_hz,
            amplitude: BASE_RMS_AMPLITUDE * f64::from(velocity),
            gate: true,
            note_generation: self.state.oscillator_input.note_generation.wrapping_add(1),
            ..self.state.oscillator_input
        };
        if input.is_valid() {
            self.state.oscillator_input = input;
            self.state.note = Some(note);
            true
        } else {
            false
        }
    }

    pub fn note_off(&mut self, note: Note) {
        if self.state.note.is_some_and(|current| {
            if current.id != -1 && note.id != -1 {
                current.id == note.id
            } else {
                current.channel == note.channel && current.pitch == note.pitch
            }
        }) {
            self.state.oscillator_input.gate = false;
            self.state.note = None;
        }
    }

    pub fn stop(&mut self) {
        // releaseとは区別し、次サンプルで即時消音する。位相は保持する。
        self.state.oscillator_input.gate = false;
        self.state.oscillator_input.reset_generation =
            self.state.oscillator_input.reset_generation.wrapping_add(1);
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
        self.state.oscillator_input.waveform = waveform;
        self.state.oscillator_input.envelope = envelope;
        self.state.oscillator_input.unison_voices = unison;
        self.state.oscillator_input.detune_cents = detune;
        self.state.oscillator_input.phase_mode = phase_mode;
    }

    /// Voice内の処理パイプライン。生成した信号をSynthesizerが加算する。
    pub fn generate_sample(&mut self, sample_rate: f64) -> f64 {
        if !(sample_rate.is_finite() & (sample_rate > 0.0)) {
            return 0.0;
        }
        let input = &self.state.oscillator_input;
        let level = self.state.cutoff_envelope.generate_sample(
            &self.state.cutoff_envelope_parameters,
            sample_rate,
            input.gate,
            input.note_generation,
            input.reset_generation,
            1.0,
        );
        let cents = level * self.state.cutoff_amount_cents;
        // バイパス中もEnvelopeは進め、有効化したサンプルで現在値の倍率を求める。
        if !matches!(self.state.filter_type, FilterType::None)
            && cents != self.state.cutoff_modulation.0
        {
            self.state.cutoff_modulation = (cents, 2.0_f64.powf(cents / 1200.0));
        }
        let sample = self
            .state
            .oscillator
            .generate_sample(&self.state.oscillator_input, sample_rate);
        let output = self.state.filter.process_sample(
            sample,
            self.state.filter_type,
            &FilterParameters {
                cutoff_hz: (self.state.oscillator_input.frequency_hz
                    * self.state.cutoff_ratio
                    * self.state.cutoff_modulation.1)
                    .min(f64::MAX),
                resonance_q: self.state.resonance_q,
            },
            sample_rate,
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
            bypassed.generate_sample(48000.0);
            reference.generate_sample(48000.0);
        }
        // 有効中に進めた参照側も、バイパス解除時と同じ無履歴から比較する。
        reference.state.filter.reset();
        bypassed.set_filter(FilterType::LowPass2, 1200.0, 2.0);
        for _ in 0..128 {
            assert_eq!(
                bypassed.generate_sample(48000.0),
                reference.generate_sample(48000.0)
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
                voice.state.oscillator_input.envelope = EnvelopeParameters {
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
                    let input = voice.state.oscillator_input;
                    let expected = filter.process_sample(
                        oscillator.generate_sample(&input, 1000.0),
                        kind,
                        &FilterParameters {
                            cutoff_hz: 220.0 * 2.0_f64.powf((-1200.0 + amount * level) / 1200.0),
                            resonance_q: 1.0,
                        },
                        1000.0,
                    );
                    assert!(
                        (voice.generate_sample(1000.0) - expected).abs() < 1e-12,
                        "sample={n}, amount={amount}, type={kind:?}"
                    );
                }
                voice.stop();
                assert_eq!(voice.generate_sample(1000.0), 0.0);
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
                let mut input = voice.state.oscillator_input;
                let mut oscillator = Oscillator::new();
                let mut reference = Filter::new();
                let settings = FilterParameters {
                    cutoff_hz: 440.0 * 2.0_f64.powf(100.0 / 1200.0) * 2.0_f64.powf(cents / 1200.0),
                    resonance_q: 2.0,
                };
                for _ in 0..64 {
                    let expected = reference.process_sample(
                        oscillator.generate_sample(&input, 48_000.0),
                        filter_type,
                        &settings,
                        48_000.0,
                    );
                    assert!((voice.generate_sample(48_000.0) - expected).abs() < 1e-12);
                }
                voice.note_off(Note {
                    id: 1,
                    channel: 0,
                    pitch: 69,
                });
                input.gate = false;
                for _ in 0..16 {
                    let expected = reference.process_sample(
                        oscillator.generate_sample(&input, 48_000.0),
                        filter_type,
                        &settings,
                        48_000.0,
                    );
                    assert!((voice.generate_sample(48_000.0) - expected).abs() < 1e-12);
                }
                voice.stop();
                assert_eq!(voice.generate_sample(48_000.0), 0.0);
            }
        }
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::envelope::{DEFAULT_ENVELOPE, EnvelopeParameters};
use crate::filter::{self, FilterParameters, FilterState, FilterType};
use crate::oscillator::{self, OscillatorInput, OscillatorState, Waveform};
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
    oscillator: OscillatorState,
    /// Note Offの照合対象。発音指示はoscillator_input.gateに反映し、発振器へノート情報を渡さない。
    note: Option<Note>,
    /// Voiceの最終段のフィルタ。ノートオフ後も残響を処理する。
    filter: FilterState,
    /// 選択中の応答と段数。Noneでは完全にバイパスする。
    filter_type: FilterType,
    /// ノート周波数に乗じる倍率。centからの換算は設定変更時に行う。
    cutoff_ratio: f64,
    /// 各段へ渡す現在のQ。積分器の履歴とは独立した入力。
    resonance_q: f64,
}

pub fn new_state() -> VoiceState {
    let values = parameters::defaults();
    VoiceState {
        oscillator_input: OscillatorInput {
            waveform: Waveform::Sine,
            unison_voices: 1,
            detune_cents: 0.0,
            frequency_hz: 440.0,
            amplitude: 0.0,
            gate: false,
            envelope: DEFAULT_ENVELOPE,
            note_generation: 0,
            reset_generation: 0,
        },
        oscillator: oscillator::new_state(),
        note: None,
        filter: filter::new_state(),
        filter_type: parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize],
        cutoff_ratio: 2.0_f64.powf(parameters::to_plain(10, values[10]) / 1200.0),
        resonance_q: parameters::to_plain(11, values[11]),
    }
}

pub fn note_on(state: &mut VoiceState, note: Note, tuning: f32, velocity: f32) -> bool {
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
        waveform: state.oscillator_input.waveform,
        frequency_hz,
        amplitude: BASE_RMS_AMPLITUDE * f64::from(velocity),
        gate: true,
        note_generation: state.oscillator_input.note_generation.wrapping_add(1),
        ..state.oscillator_input
    };
    if input.is_valid() {
        state.oscillator_input = input;
        state.note = Some(note);
        true
    } else {
        false
    }
}

pub fn note_off(state: &mut VoiceState, note: Note) {
    if state.note.is_some_and(|current| {
        if current.id != -1 && note.id != -1 {
            current.id == note.id
        } else {
            current.channel == note.channel && current.pitch == note.pitch
        }
    }) {
        state.oscillator_input.gate = false;
        state.note = None;
    }
}

pub fn stop(state: &mut VoiceState) {
    // releaseとは区別し、次サンプルで即時消音する。位相は保持する。
    state.oscillator_input.gate = false;
    state.oscillator_input.reset_generation =
        state.oscillator_input.reset_generation.wrapping_add(1);
    state.note = None;
    filter::reset(&mut state.filter);
}

pub fn set_filter(
    state: &mut VoiceState,
    filter_type: FilterType,
    cutoff_cents: f64,
    resonance_q: f64,
) {
    state.filter_type = filter_type;
    state.cutoff_ratio = 2.0_f64.powf(cutoff_cents / 1200.0);
    state.resonance_q = resonance_q;
}

pub fn set_parameters(
    state: &mut VoiceState,
    waveform: Waveform,
    envelope: EnvelopeParameters,
    unison: u32,
    detune: f64,
) {
    state.oscillator_input.waveform = waveform;
    state.oscillator_input.envelope = envelope;
    state.oscillator_input.unison_voices = unison;
    state.oscillator_input.detune_cents = detune;
}

/// Voice内の処理パイプライン。生成した信号をSynthesizerが加算する。
pub fn generate_sample(state: &mut VoiceState, sample_rate: f64) -> f64 {
    let sample =
        oscillator::generate_sample(&mut state.oscillator, &state.oscillator_input, sample_rate);
    filter::process_sample(
        &mut state.filter,
        sample,
        state.filter_type,
        &FilterParameters {
            cutoff_hz: (state.oscillator_input.frequency_hz * state.cutoff_ratio).min(f64::MAX),
            resonance_q: state.resonance_q,
        },
        sample_rate,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoff_tracks_note_tuning_and_filters_after_unison() {
        for filter_type in parameters::FILTER_TYPES {
            for cents in [-2400.0, 1200.0, 9600.0] {
                let mut voice = new_state();
                set_parameters(
                    &mut voice,
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
                );
                set_filter(&mut voice, filter_type, cents, 2.0);
                assert!(note_on(
                    &mut voice,
                    Note {
                        id: 1,
                        channel: 0,
                        pitch: 69
                    },
                    100.0,
                    1.0
                ));
                let mut input = voice.oscillator_input;
                let mut oscillator = oscillator::new_state();
                let mut reference = filter::new_state();
                let settings = FilterParameters {
                    cutoff_hz: 440.0 * 2.0_f64.powf(100.0 / 1200.0) * 2.0_f64.powf(cents / 1200.0),
                    resonance_q: 2.0,
                };
                for _ in 0..64 {
                    let expected = filter::process_sample(
                        &mut reference,
                        oscillator::generate_sample(&mut oscillator, &input, 48_000.0),
                        filter_type,
                        &settings,
                        48_000.0,
                    );
                    assert!((generate_sample(&mut voice, 48_000.0) - expected).abs() < 1e-12);
                }
                note_off(
                    &mut voice,
                    Note {
                        id: 1,
                        channel: 0,
                        pitch: 69,
                    },
                );
                input.gate = false;
                for _ in 0..16 {
                    let expected = filter::process_sample(
                        &mut reference,
                        oscillator::generate_sample(&mut oscillator, &input, 48_000.0),
                        filter_type,
                        &settings,
                        48_000.0,
                    );
                    assert!((generate_sample(&mut voice, 48_000.0) - expected).abs() < 1e-12);
                }
                stop(&mut voice);
                assert_eq!(generate_sample(&mut voice, 48_000.0), 0.0);
            }
        }
    }
}

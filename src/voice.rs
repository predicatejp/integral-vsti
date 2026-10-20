// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::envelope::{DEFAULT_ENVELOPE, EnvelopeParameters};
use crate::oscillator::{Oscillator, OscillatorInput, Waveform};

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
pub struct Voice {
    /// このVoice専用の発振器。現在は1個で、内部でユニゾンを生成する。
    pub(super) oscillator: Oscillator,
    /// Note Offの照合対象。発音指示はoscillator.input.gateに反映し、発振器へノート情報を渡さない。
    note: Option<Note>,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            oscillator: Oscillator::new(OscillatorInput {
                waveform: Waveform::Sine,
                unison_voices: 1,
                detune_cents: 0.0,
                frequency_hz: 440.0,
                amplitude: 0.0,
                gate: false,
                envelope: DEFAULT_ENVELOPE,
                note_generation: 0,
                reset_generation: 0,
            }),
            note: None,
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
            waveform: self.oscillator.input.waveform,
            frequency_hz,
            amplitude: BASE_RMS_AMPLITUDE * f64::from(velocity),
            gate: true,
            note_generation: self.oscillator.input.note_generation.wrapping_add(1),
            ..self.oscillator.input
        };
        if input.is_valid() {
            self.oscillator.input = input;
            self.note = Some(note);
            true
        } else {
            false
        }
    }

    pub fn note_off(&mut self, note: Note) {
        if self.note.is_some_and(|current| {
            if current.id != -1 && note.id != -1 {
                current.id == note.id
            } else {
                current.channel == note.channel && current.pitch == note.pitch
            }
        }) {
            self.oscillator.input.gate = false;
            self.note = None;
        }
    }

    pub fn stop(&mut self) {
        // releaseとは区別し、次サンプルで即時消音する。位相は保持する。
        self.oscillator.input.gate = false;
        self.oscillator.input.reset_generation =
            self.oscillator.input.reset_generation.wrapping_add(1);
        self.note = None;
    }

    pub fn set_parameters(
        &mut self,
        waveform: Waveform,
        envelope: EnvelopeParameters,
        unison: u32,
        detune: f64,
    ) {
        self.oscillator.input.waveform = waveform;
        self.oscillator.input.envelope = envelope;
        self.oscillator.input.unison_voices = unison;
        self.oscillator.input.detune_cents = detune;
    }

    /// Voice内の処理パイプライン。生成した信号をSynthesizerが加算する。
    pub fn next_sample(&mut self, sample_rate: f64) -> f64 {
        self.oscillator.next_sample(sample_rate)
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::oscillator::{Oscillator, OscillatorParams};

#[derive(Clone, Copy)]
pub struct Note {
    pub id: i32,
    pub channel: i16,
    pub pitch: i16,
}

pub struct Synth {
    oscillator: Oscillator,
    note: Option<Note>,
    sample_rate: f64,
}

impl Synth {
    pub fn new() -> Self {
        Self {
            oscillator: Oscillator::new(),
            note: None,
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
    }

    pub fn note_on(&mut self, note: Note, tuning: f32, velocity: f32) {
        if !(0..16).contains(&note.channel)
            || !(0..128).contains(&note.pitch)
            || !tuning.is_finite()
            || !(0.0..=1.0).contains(&velocity)
        {
            return;
        }
        let frequency_hz =
            440.0 * 2.0_f64.powf((f64::from(note.pitch) - 69.0 + f64::from(tuning) / 100.0) / 12.0);
        if self.oscillator.set_params(OscillatorParams {
            frequency_hz,
            amplitude: 0.2 * f64::from(velocity),
        }) {
            self.note = Some(note);
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
            self.stop();
        }
    }

    pub fn stop(&mut self) {
        // ゲートだけを閉じる。ノートオフや処理停止でオシレータを作り直さない。
        self.note = None;
    }

    pub fn next_sample(&mut self) -> f64 {
        if self.note.is_some() {
            self.oscillator.next_sample(self.sample_rate)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: i32, channel: i16, pitch: i16) -> Note {
        Note { id, channel, pitch }
    }

    #[test]
    fn latest_note_wins_and_retrigger_inherits_phase_after_silence() {
        let mut synth = Synth::new();
        synth.set_sample_rate(1760.0);
        synth.note_on(note(1, 0, 69), 0.0, 1.0);
        assert_eq!(synth.next_sample(), 0.0); // 位相0 -> 1/4
        synth.note_on(note(2, 0, 81), 0.0, 0.5);
        synth.note_off(note(1, 0, 69)); // 古いノートオフは無視
        assert!((synth.next_sample() - 0.1).abs() < 1e-14); // 位相1/4 -> 3/4
        synth.note_off(note(2, 0, 81));
        for _ in 0..7 {
            assert_eq!(synth.next_sample(), 0.0);
        }
        synth.note_on(note(3, 0, 69), 0.0, 1.0);
        assert!((synth.next_sample() + 0.2).abs() < 1e-14); // 消音前の位相3/4から再開
        synth.stop();
        assert_eq!(synth.next_sample(), 0.0);
    }

    #[test]
    fn missing_note_ids_match_channel_and_pitch_and_invalid_notes_are_ignored() {
        let mut synth = Synth::new();
        synth.set_sample_rate(1760.0);
        synth.note_on(note(-1, 2, 69), 0.0, 1.0);
        synth.next_sample();
        synth.note_off(note(-1, 3, 69));
        synth.note_off(note(-1, 2, 70));
        synth.note_on(note(10, 16, 69), 0.0, 1.0);
        synth.note_on(note(10, 0, 128), 0.0, 1.0);
        synth.note_on(note(10, 0, 69), f32::MAX, 1.0);
        synth.note_on(note(10, 0, 69), 0.0, f32::NAN);
        assert!((synth.next_sample() - 0.2).abs() < 1e-14);
        synth.note_off(note(-1, 2, 69));
        assert_eq!(synth.next_sample(), 0.0);
    }
}

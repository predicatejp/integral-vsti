// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::oscillator::{OscillatorInput, OscillatorState, Waveform};

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

pub struct Synth {
    /// 発振器へ継続的に与える波形・周波数・目標実効値・ゲート。ノートイベントはこの入力だけを更新する。
    oscillator_input: OscillatorInput,
    /// 発振器の内部状態。サンプル生成後にだけ更新し、ノート切り替え時にも作り直さない。
    oscillator_state: OscillatorState,
    /// Note Offの照合対象。発音指示はoscillator_input.gateに反映し、発振器へノート情報を渡さない。
    note: Option<Note>,
    /// 波形生成に用いるサンプルレート（Hz）。初期値44100、処理設定でホストの値へ更新する。
    sample_rate: f64,
}

impl Synth {
    pub fn new() -> Self {
        Self {
            oscillator_input: OscillatorInput {
                waveform: Waveform::Sine,
                frequency_hz: 440.0,
                amplitude: 0.0,
                gate: false,
            },
            oscillator_state: OscillatorState::new(),
            note: None,
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
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
            waveform: self.oscillator_input.waveform,
            frequency_hz,
            amplitude: BASE_RMS_AMPLITUDE * f64::from(velocity),
            gate: true,
        };
        if input.is_valid() {
            self.oscillator_input = input;
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
            self.stop();
        }
    }

    pub fn stop(&mut self) {
        // 定常入力のゲートだけを閉じ、発振器の内部状態には触れない。
        self.oscillator_input.gate = false;
        self.note = None;
    }

    pub fn next_sample(&mut self) -> f64 {
        self.oscillator_state
            .next_sample(&self.oscillator_input, self.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: i32, channel: i16, pitch: i16) -> Note {
        Note { id, channel, pitch }
    }

    #[test]
    fn every_waveform_has_half_scale_rms_at_maximum_velocity() {
        // 通常は各レートで440 Hzを1秒間測定。Miriでは256点で1周期となる条件に縮小する。
        let measurements: &[(i32, i32)] = if cfg!(miri) {
            &[(112640, 256)]
        } else {
            &[(44100, 44100), (48000, 48000), (96000, 96000)]
        };
        for waveform in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for &(rate, samples) in measurements {
                for velocity in [0.0, 0.25, 0.5, 1.0] {
                    let mut synth = Synth::new();
                    synth.oscillator_input.waveform = waveform;
                    synth.set_sample_rate(f64::from(rate));
                    assert!(synth.note_on(note(1, 0, 69), 0.0, velocity));
                    let mut energy = 0.0;
                    for _ in 0..samples {
                        let sample = synth.next_sample();
                        assert!(sample.is_finite() && sample.abs() <= 1.0);
                        energy += sample * sample;
                    }
                    let rms = (energy / f64::from(samples)).sqrt();
                    // 理想波形のRMSに対し、有限個の離散サンプルによる誤差を許容する。
                    assert!(
                        (rms - 0.5 * f64::from(velocity)).abs() < 1e-4,
                        "{waveform:?}, rate={rate}, velocity={velocity}, rms={rms}"
                    );
                    synth.note_off(note(1, 0, 69));
                    for _ in 0..8 {
                        assert_eq!(synth.next_sample(), 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn latest_note_wins_and_retrigger_inherits_phase_after_silence() {
        let mut synth = Synth::new();
        synth.set_sample_rate(1760.0);
        synth.note_on(note(1, 0, 69), 0.0, 1.0);
        assert_eq!(synth.next_sample(), 0.0); // 位相0 -> 1/4
        synth.note_on(note(2, 0, 81), 0.0, 0.5);
        synth.note_off(note(1, 0, 69)); // 古いノートオフは無視
        assert!((synth.next_sample() - 0.25 * std::f64::consts::SQRT_2).abs() < 1e-14); // 位相1/4 -> 3/4
        synth.note_off(note(2, 0, 81));
        for _ in 0..7 {
            assert_eq!(synth.next_sample(), 0.0);
        }
        synth.note_on(note(3, 0, 69), 0.0, 1.0);
        assert!((synth.next_sample() + 0.5 * std::f64::consts::SQRT_2).abs() < 1e-14); // 消音前の位相3/4から再開
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
        assert!((synth.next_sample() - 0.5 * std::f64::consts::SQRT_2).abs() < 1e-14);
        synth.note_off(note(-1, 2, 69));
        assert_eq!(synth.next_sample(), 0.0);
    }
}

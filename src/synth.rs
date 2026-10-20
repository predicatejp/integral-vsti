// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::parameters;
use crate::voice::{self, Note, VoiceState};

pub struct SynthesizerState {
    /// 最大32枠を初期化時に確保し、音声処理中は確保・解放しない。
    voices: [VoiceState; 32],
    /// 次の有効なNote Onを割り当てるオシレータID。初期値0、使用枠数で循環する。
    next_oscillator_id: usize,
    /// 現在使用する枠数（1..=32）。範囲外の枠は停止し、位相だけ保持する。
    polyphony: usize,
    /// 全オシレータに共通のサンプルレート（Hz）。
    sample_rate: f64,
    /// 適用済みの正規化値。変更時だけ実単位へ換算する。
    parameters: [f64; 12],
}

pub fn new_state() -> SynthesizerState {
    SynthesizerState {
        voices: std::array::from_fn(|_| voice::new_state()),
        next_oscillator_id: 0,
        polyphony: 1,
        sample_rate: 44100.0,
        parameters: parameters::defaults(),
    }
}

pub fn set_sample_rate(state: &mut SynthesizerState, sample_rate: f64) {
    state.sample_rate = sample_rate;
}

/// 設定は全枠の入力に反映し、発振器の内部状態はイベント処理から変更しない。
pub fn set_parameters(state: &mut SynthesizerState, values: [f64; 12]) -> bool {
    if values == state.parameters {
        return false;
    }
    state.parameters = values;
    let count = parameters::to_plain(8, values[8]) as usize;
    if count < state.polyphony {
        for voice in &mut state.voices[count..state.polyphony] {
            voice::stop(voice);
        }
    }
    state.polyphony = count;
    state.next_oscillator_id %= count;
    let waveform = parameters::from_normalized(values[0]).expect("validated values");
    let envelope = parameters::envelope(&values);
    let unison_voices = parameters::to_plain(6, values[6]) as u32;
    let detune_cents = parameters::to_plain(7, values[7]);
    for voice in &mut state.voices {
        voice::set_parameters(voice, waveform, envelope, unison_voices, detune_cents);
        voice::set_filter(
            voice,
            parameters::FILTER_TYPES[parameters::to_plain(9, values[9]) as usize],
            parameters::to_plain(10, values[10]),
            parameters::to_plain(11, values[11]),
        );
    }
    true
}

pub fn note_on(state: &mut SynthesizerState, note: Note, tuning: f32, velocity: f32) -> bool {
    // 空き枠探索はせず、指定された循環順で発音中の枠も再使用する。
    if !voice::note_on(
        &mut state.voices[state.next_oscillator_id],
        note,
        tuning,
        velocity,
    ) {
        return false;
    }
    state.next_oscillator_id = (state.next_oscillator_id + 1) % state.polyphony;
    true
}

pub fn note_off(state: &mut SynthesizerState, note: Note) {
    // 同じID、またはID未提供の同チャンネル・同音高が複数あれば全一致をreleaseする。
    for voice in &mut state.voices[..state.polyphony] {
        voice::note_off(voice, note);
    }
}

pub fn stop(state: &mut SynthesizerState) {
    for voice in &mut state.voices {
        voice::stop(voice);
    }
}

pub fn generate_sample(state: &mut SynthesizerState) -> f64 {
    state.voices[..state.polyphony]
        .iter_mut()
        .map(|voice| voice::generate_sample(voice, state.sample_rate))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::DEFAULT_ENVELOPE;
    use crate::oscillator::Waveform;

    // 既存の発振器・ノート処理の期待値はフィルタをバイパスして検証する。
    fn unfiltered() -> SynthesizerState {
        let mut synth = new_state();
        let mut values = parameters::defaults();
        values[9] = 0.0;
        set_parameters(&mut synth, values);
        synth
    }

    fn note(id: i32, channel: i16, pitch: i16) -> Note {
        Note { id, channel, pitch }
    }

    fn polyphonic(count: usize) -> SynthesizerState {
        let mut synth = unfiltered();
        let mut values = parameters::defaults();
        values[9] = 0.0;
        values[0] = parameters::normalized(Waveform::Square);
        values[1] = 0.0;
        values[3] = 0.0;
        values[4] = 1.0;
        values[5] = 0.0;
        values[8] = parameters::to_normalized(8, count as f64);
        set_parameters(&mut synth, values);
        set_sample_rate(&mut synth, 48000.0);
        synth
    }

    #[test]
    fn filtered_voices_sum_independently_and_stop_without_tail() {
        for filter_type in 0..7 {
            let mut synth = new_state();
            let mut values = parameters::defaults();
            values[1] = 0.0;
            values[3] = 0.0;
            values[4] = 1.0;
            values[5] = 0.0;
            values[8] = parameters::to_normalized(8, 2.0);
            values[9] = parameters::to_normalized(9, f64::from(filter_type));
            values[10] = parameters::to_normalized(10, -1200.0);
            values[11] = parameters::to_normalized(11, 3.0);
            set_parameters(&mut synth, values);
            let mut references: [SynthesizerState; 2] = std::array::from_fn(|_| {
                let mut single = new_state();
                let mut values = values;
                values[8] = 0.0;
                set_parameters(&mut single, values);
                single
            });
            for n in 0..96 {
                for (at, slot, id, pitch) in [(0, 0, 1, 57), (3, 1, 2, 81), (32, 0, 3, 69)] {
                    if n == at {
                        note_on(&mut synth, note(id, 0, pitch), 0.0, 1.0);
                        note_on(&mut references[slot], note(id, 0, pitch), 0.0, 1.0);
                    }
                }
                if n == 48 {
                    note_off(&mut synth, note(2, 0, 81));
                    note_off(&mut references[1], note(2, 0, 81));
                }
                if n == 64 {
                    values[8] = 0.0;
                    set_parameters(&mut synth, values);
                    stop(&mut references[1]);
                }
                let expected: f64 = references.iter_mut().map(generate_sample).sum();
                assert!((generate_sample(&mut synth) - expected).abs() < 1e-12);
            }
            stop(&mut synth);
            assert_eq!(generate_sample(&mut synth), 0.0);
        }
    }

    #[test]
    fn polyphony_cycles_all_counts_and_sums_independent_notes() {
        for count in 1..=32 {
            let mut synth = polyphonic(count);
            for id in 0..count {
                assert_eq!(synth.next_oscillator_id, id);
                assert!(note_on(
                    &mut synth,
                    note(id as i32, 0, 60 + id as i16),
                    0.0,
                    1.0
                ));
            }
            assert_eq!(synth.next_oscillator_id, 0);
            // 全枠の初回サンプルは矩形波の正側。音数による音量補正はしない。
            assert!((generate_sample(&mut synth) - 0.5 * count as f64).abs() < 1e-12);
            assert!(note_on(&mut synth, note(100, 0, 100), 0.0, 0.5));
            note_off(&mut synth, note(0, 0, 60)); // 上書き済みのノートでは新しい音を消さない。
            assert!(synth.voices[0].oscillator_input.gate);
            assert!((generate_sample(&mut synth) - (0.5 * count as f64 - 0.25)).abs() < 1e-12);
            let next = synth.next_oscillator_id;
            note_off(&mut synth, note(100, 0, 100));
            assert_eq!(synth.next_oscillator_id, next);
            for id in 1..count {
                note_off(&mut synth, note(id as i32, 0, 60 + id as i16));
            }
            assert_eq!(generate_sample(&mut synth), 0.0);
            assert!(!note_on(&mut synth, note(200, 16, 60), 0.0, 1.0));
            assert!(!note_on(&mut synth, note(200, 0, 60), f32::MAX, 1.0));
            assert_eq!(synth.next_oscillator_id, next);
        }
    }

    #[test]
    fn polyphony_envelopes_and_unison_match_independent_oscillators() {
        let mut synth = polyphonic(3);
        let mut values = synth.parameters;
        values[0] = 0.0; // Sineで位相差も比較する。
        values[1] = parameters::to_normalized(1, 5.0);
        values[5] = parameters::to_normalized(5, 10.0);
        values[6] = 1.0;
        values[7] = 0.37;
        set_parameters(&mut synth, values);
        let mut references: [SynthesizerState; 3] = std::array::from_fn(|_| {
            let mut single = polyphonic(1);
            let mut settings = values;
            settings[8] = 0.0;
            set_parameters(&mut single, settings);
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
                    note_on(&mut synth, note(id, 0, pitch), 0.0, velocity);
                    note_on(&mut references[slot], note(id, 0, pitch), 0.0, velocity);
                }
            }
            if sample == 32 {
                note_off(&mut synth, note(2, 0, 64));
                note_off(&mut references[1], note(2, 0, 64));
            }
            if sample == 64 {
                note_off(&mut synth, note(1, 0, 60));
            } // 0番の新しい音へ影響しない。
            if sample == 80 {
                stop(&mut synth);
                for single in &mut references {
                    stop(single);
                }
            }
            let expected: f64 = references.iter_mut().map(generate_sample).sum();
            assert!((generate_sample(&mut synth) - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn polyphony_resize_stops_removed_notes_and_bounds_next_id() {
        let mut synth = polyphonic(4);
        for id in 0..3 {
            note_on(&mut synth, note(id, 0, 60), 0.0, 1.0);
        }
        assert_eq!(generate_sample(&mut synth), 1.5);
        let mut values = synth.parameters;
        values[8] = parameters::to_normalized(8, 2.0);
        set_parameters(&mut synth, values);
        assert_eq!(synth.next_oscillator_id, 1); // 3 % 2
        assert_eq!(generate_sample(&mut synth), 1.0);
        values[8] = parameters::to_normalized(8, 4.0);
        set_parameters(&mut synth, values);
        assert_eq!(generate_sample(&mut synth), 1.0); // 除外したノートは復活しない。
        note_on(&mut synth, note(4, 0, 60), 0.0, 1.0); // 1番を上書き。
        note_on(&mut synth, note(5, 0, 60), 0.0, 1.0); // 2番を再使用。
        assert_eq!(generate_sample(&mut synth), 1.5);
        // サンプル処理を挟まず減少→増加→再割り当てしても古い振幅を引き継がない。
        values[8] = 0.0;
        set_parameters(&mut synth, values);
        values[8] = parameters::to_normalized(8, 4.0);
        values[1] = parameters::to_normalized(1, 10.0);
        set_parameters(&mut synth, values);
        note_on(&mut synth, note(6, 0, 60), 0.0, 1.0);
        note_on(&mut synth, note(7, 0, 60), 0.0, 1.0);
        assert_eq!(generate_sample(&mut synth), 0.5);
        stop(&mut synth);
        assert_eq!(generate_sample(&mut synth), 0.0);
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
            note_on(&mut synth, n, 0.0, 1.0);
        }
        assert_eq!(generate_sample(&mut synth), 2.0);
        note_off(&mut synth, note(1, 0, 60));
        assert_eq!(generate_sample(&mut synth), 1.5);
        note_off(&mut synth, note(-1, 2, 60)); // 別チャンネルは無関係。
        assert_eq!(generate_sample(&mut synth), 1.5);
        note_off(&mut synth, note(-1, 1, 60)); // IDなし同音の全一致をrelease。
        assert_eq!(generate_sample(&mut synth), 0.5);
        note_off(&mut synth, note(2, 0, 60));
        assert_eq!(generate_sample(&mut synth), 0.0);
        assert_eq!(synth.next_oscillator_id, 0);
    }

    #[test]
    fn every_waveform_has_expected_sustain_rms() {
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
                    let mut synth = unfiltered();
                    synth.voices[0].oscillator_input.waveform = waveform;
                    // 定常RMSの検証なので、立ち上がり・リリースだけ許容最短にする。
                    synth.voices[0].oscillator_input.envelope.attack_ms = 1.0;
                    synth.voices[0].oscillator_input.envelope.decay_ms = 1.0;
                    synth.voices[0].oscillator_input.envelope.release_ms = 1.0;
                    set_sample_rate(&mut synth, f64::from(rate));
                    assert!(note_on(&mut synth, note(1, 0, 69), 0.0, velocity));
                    // attack/decayが終わったsustain区間のRMSを測る。
                    for _ in 0..(f64::from(rate) * 0.003).ceil() as usize {
                        generate_sample(&mut synth);
                    }
                    let mut energy = 0.0;
                    for _ in 0..samples {
                        let sample = generate_sample(&mut synth);
                        assert!(sample.is_finite() && sample.abs() <= 1.0);
                        energy += sample * sample;
                    }
                    let rms = (energy / f64::from(samples)).sqrt();
                    // 理想波形のRMSに対し、有限個の離散サンプルによる誤差を許容する。
                    assert!(
                        (rms - 0.5 * DEFAULT_ENVELOPE.sustain * f64::from(velocity)).abs() < 1e-4,
                        "{waveform:?}, rate={rate}, velocity={velocity}, rms={rms}"
                    );
                    note_off(&mut synth, note(1, 0, 69));
                    for _ in 0..(f64::from(rate) * 0.002).ceil() as usize {
                        generate_sample(&mut synth);
                    }
                    for _ in 0..8 {
                        assert_eq!(generate_sample(&mut synth), 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn retrigger_release_and_stop_preserve_amplitude_and_phase() {
        let mut synth = unfiltered();
        set_sample_rate(&mut synth, 1000.0); // 1サンプル=1ms。
        synth.voices[0].oscillator_input.waveform = Waveform::Square;
        note_on(&mut synth, note(1, 0, 69), 0.0, 1.0);
        for i in 0..5 {
            assert!((generate_sample(&mut synth).abs() - 0.05 * f64::from(i)).abs() < 1e-12);
        }
        // attack中の0.25から、異なるベロシティの目標0.4まで10msで移行。
        note_on(&mut synth, note(2, 0, 81), 0.0, 0.8);
        note_off(&mut synth, note(1, 0, 69)); // 古いノートではreleaseしない。
        let peak = 0.5 * f64::from(0.8_f32);
        for i in 0..=10 {
            let expected = 0.25 + (peak - 0.25) * f64::from(i) / 10.0;
            assert!((generate_sample(&mut synth).abs() - expected).abs() < 1e-12);
        }
        note_off(&mut synth, note(2, 0, 81));
        let start = peak * 0.997; // decayを1ms進めた位置。
        for i in 0..=100 {
            assert!(
                (generate_sample(&mut synth).abs() - start * (1.0 - f64::from(i) / 100.0)).abs()
                    < 1e-12
            );
        }
        assert_eq!(generate_sample(&mut synth), 0.0);
        note_on(&mut synth, note(3, 0, 69), 0.0, 1.0);
        assert_eq!(generate_sample(&mut synth), 0.0);
        generate_sample(&mut synth);
        // stopの直後、サンプル処理前にNote Onが来ても古い音量を引き継がない。
        stop(&mut synth);
        note_on(&mut synth, note(4, 0, 69), 0.0, 1.0);
        assert_eq!(generate_sample(&mut synth), 0.0);
        stop(&mut synth);
        assert_eq!(generate_sample(&mut synth), 0.0);
    }

    #[test]
    fn missing_note_ids_match_channel_and_pitch_and_invalid_notes_are_ignored() {
        let mut synth = unfiltered();
        set_sample_rate(&mut synth, 1000.0);
        synth.voices[0].oscillator_input.waveform = Waveform::Square;
        note_on(&mut synth, note(-1, 2, 69), 0.0, 1.0);
        for _ in 0..5 {
            generate_sample(&mut synth);
        }
        note_off(&mut synth, note(-1, 3, 69));
        note_off(&mut synth, note(-1, 2, 70));
        note_on(&mut synth, note(10, 16, 69), 0.0, 1.0);
        note_on(&mut synth, note(10, 0, 128), 0.0, 1.0);
        note_on(&mut synth, note(10, 0, 69), f32::MAX, 1.0);
        note_on(&mut synth, note(10, 0, 69), 0.0, f32::NAN);
        assert!((generate_sample(&mut synth).abs() - 0.25).abs() < 1e-12);
        note_off(&mut synth, note(-1, 2, 69));
        assert!((generate_sample(&mut synth).abs() - 0.3).abs() < 1e-12);
        for _ in 0..100 {
            generate_sample(&mut synth);
        }
        assert_eq!(generate_sample(&mut synth), 0.0);
    }
}

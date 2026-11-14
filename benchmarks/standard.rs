// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! 通常の演奏・音色・オートメーションを固定した性能回帰スイート。
#![allow(dead_code)] // ABI・保存処理など、計測対象以外の既存定義も読み込む。
#[path = "../src/abi.rs"]
mod abi;
#[path = "../src/chorus.rs"]
mod chorus;
#[path = "../src/compressor.rs"]
mod compressor;
#[path = "../src/delay.rs"]
mod delay;
#[path = "../src/distortion.rs"]
mod distortion;
#[path = "../src/effects.rs"]
mod effects;
#[path = "../src/envelope.rs"]
mod envelope;
#[path = "../src/eq.rs"]
mod eq;
#[path = "../src/filter.rs"]
mod filter;
#[path = "../src/float_mode.rs"]
mod float_mode;
#[path = "../src/limiter.rs"]
mod limiter;
#[path = "../src/oscillator.rs"]
mod oscillator;
#[path = "../src/parameters.rs"]
mod parameters;
#[path = "../src/phaser.rs"]
mod phaser;
#[path = "../src/random.rs"]
mod random;
#[path = "../src/reverb.rs"]
mod reverb;
#[path = "../src/synth.rs"]
mod synth;
#[path = "../src/voice.rs"]
mod voice;
#[path = "../src/volume.rs"]
mod volume;

use std::{
    hint::black_box,
    time::{Duration, Instant},
};
use voice::Note;

// 設定はUIの初期値から独立させ、新パラメーターの追加時には明示的に見直す。
#[rustfmt::skip]
const BASE: [f64; parameters::COUNT] = [
    0., // Waveform
    10., 0., 120., 0.7, 100., // Amp A/H/D/S/R
    1., 0., 1., // Unison / Detune / Polyphony
    0., 3600., 1., 1., -6., // Filter / Cutoff / Q / Phase Zero / Master
    10., 0., 100., 0.7, 100., // Cutoff A/H/D/S/R
    0., 0., 0., // Amount / Width / Pan
    0., // Osc1 Vol
    0., 0., f64::NEG_INFINITY, // Osc2 Waveform / Tune / Vol
    0., 0., f64::NEG_INFINITY, // Osc3 Waveform / Tune / Vol
    0., 0., // Mod 2 / 3
    0., 0., 100., // Distortion 1 Clipper / Gain / Mix
    0.0, 250.0, 0.0, 1000.0, 0.0, 1.0, 4000.0, 0.0, // EQ Enable / Low F,G / Mid F,G,Q / High F,G
    0.0, 0.0, 0.0, // Slot 1 / 2 / 3: Off
    0.0, 0.0, 100.0, 0.0, 0.0, 100.0, // Distortion 2 / 3
    5.0, 35.0, 25.0, 5.0, 35.0, 25.0, 5.0, 35.0, 25.0, // Delay Time / Feedback / Mix x3
    0.0, 0.0, 0.0, // Delay Stereo x3
    2.0, 6000.0, 100.0, 25.0, // Reverb 1 Decay / Damping / Size / Mix
    2.0, 6000.0, 100.0, 25.0, // Reverb 2
    2.0, 6000.0, 100.0, 25.0, // Reverb 3
    20., 2., 0.7, 0., 0., 50., // Chorus 1 Time / Depth / Rate / Detune / Feedback / Mix
    20., 2., 0.7, 0., 0., 50., // Chorus 2
    20., 2., 0.7, 0., 0., 50., // Chorus 3
    1000., 1200., 0.5, 50., 50., // Phaser 1 Center / Depth / Rate / Feedback / Mix
    1000., 1200., 0.5, 50., 50., // Phaser 2
    1000., 1200., 0.5, 50., 50., // Phaser 3
    0., // Limiter Off
    -18., 4., 10., 100., 6., 0., 100., // Compressor 1 Threshold / Ratio / Attack / Release / Knee / Makeup / Mix
    -18., 4., 10., 100., 6., 0., 100., // Compressor 2
    -18., 4., 10., 100., 6., 0., 100., // Compressor 3
    100.0, 0.0, // Amp / Filter velocity sensitivity
    0.0, 200.0, // Portamento / Pitch
];
const BASS: &[(usize, f64)] = &[
    (0, 3.),
    (1, 5.),
    (3, 180.),
    (4, 0.45),
    (9, 2.),
    (10, 1200.),
    (11, 1.2),
    (14, 0.),
    (16, 250.),
    (17, 0.),
    (19, 2400.),
];
const LEAD: &[(usize, f64)] = &[(0, 2.), (6, 2.), (7, 7.), (9, 2.), (10, 2400.), (11, 0.9)];
const CHORD: &[(usize, f64)] = &[
    (0, 3.),
    (1, 15.),
    (3, 200.),
    (4, 0.6),
    (5, 400.),
    (6, 3.),
    (7, 9.),
    (8, 8.),
    (9, 2.),
    (11, 0.8),
    (20, 60.),
];
const PAD: &[(usize, f64)] = &[
    (0, 3.),
    (1, 300.),
    (3, 500.),
    (5, 800.),
    (6, 5.),
    (7, 12.),
    (8, 8.),
    (9, 2.),
    (10, 2400.),
    (14, 600.),
    (16, 500.),
    (19, 1200.),
    (20, 80.),
];
const BELL: &[(usize, f64)] = &[
    (1, 2.),
    (3, 1000.),
    (4, 0.),
    (5, 300.),
    (8, 4.),
    (24, 1200.),
    (27, 1900.),
    (29, 30.),
    (30, 10.),
];

#[derive(Clone, Copy)]
enum Phrase {
    Mono,
    Legato,
    Tail,
    Bass,
    Arpeggio,
    Chord,
    Idle,
}
#[derive(Clone, Copy)]
struct Automation {
    index: usize,
    low: f64,
    high: f64,
    period_seconds: f64,
}
struct Case {
    name: &'static str,
    phrase: Phrase,
    settings: &'static [(usize, f64)],
    extra: &'static [(usize, f64)],
    automation: Option<Automation>,
}

impl Case {
    fn new(
        name: &'static str,
        phrase: Phrase,
        settings: &'static [(usize, f64)],
        extra: &'static [(usize, f64)],
    ) -> Self {
        Self {
            name,
            phrase,
            settings,
            extra,
            automation: None,
        }
    }
}

// 全体系列: Voice合成 → Eff1/2/3 → EQ → Limiter → Master。
// 種類は各ケースで明示する。内部パラメーターの存在から暗黙に有効化しない。
const SOFT: &[(usize, f64)] = &[(42, 4.), (31, 0.), (32, 9.), (33, 40.)];
const CHORUS: &[(usize, f64)] = &[
    (42, 1.),
    (75, 18.),
    (76, 2.5),
    (77, 0.6),
    (78, 0.03),
    (80, 35.),
];
const DELAY: &[(usize, f64)] = &[(42, 3.), (51, 5.), (52, 35.), (53, 25.)];
const REVERB: &[(usize, f64)] = &[(42, 6.), (63, 2.5), (64, 5000.), (65, 110.), (66, 25.)];
const EQ: &[(usize, f64)] = &[
    (34, 1.),
    (35, 180.),
    (36, 3.),
    (37, 1200.),
    (38, -4.),
    (39, 1.2),
    (40, 6000.),
    (41, 2.),
];

#[rustfmt::skip]
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case::new("sine-mono", Phrase::Mono, &[], &[]),
        Case::new("saw-bass", Phrase::Bass, BASS, &[(131, 75.)]),
        Case::new("triangle-pluck", Phrase::Arpeggio,
            &[(0, 1.), (1, 2.), (3, 250.), (4, 0.), (5, 180.), (8, 4.), (9, 1.)], &[]),
        Case::new("square-lead", Phrase::Mono, LEAD, &[]),
        Case::new("saw-chords", Phrase::Chord, CHORD, &[]),
        Case::new("slow-pad", Phrase::Chord, PAD, &[]),
        Case::new("three-osc-keys", Phrase::Chord,
            &[(0, 3.), (5, 400.), (8, 8.), (9, 1.), (22, -6.), (23, 1.), (24, -1200.), (25, -12.), (28, -18.)], &[]),
        Case::new("phase-mod-bell", Phrase::Arpeggio, BELL, &[]),
        Case::new("portamento-lead", Phrase::Legato, LEAD, &[(12, 0.), (parameters::PORTAMENTO, 120.)]),
        Case::new("soft-bass", Phrase::Bass, BASS, SOFT),
        Case::new("hard-lead", Phrase::Mono, LEAD, &[(42, 4.), (31, 1.), (32, 6.), (33, 25.)]),
        Case::new("compressor-bass", Phrase::Bass, BASS, &[(42, 2.)]),
        Case::new("compressor-chords", Phrase::Chord, CHORD, &[(42, 2.)]),
        Case::new("delay-lead", Phrase::Mono, LEAD, DELAY),
        Case::new("ping-pong-keys", Phrase::Arpeggio, BELL, &[(42, 3.), (51, 7.), (52, 40.), (53, 30.), (60, 2.)]),
        Case::new("reverb-pad", Phrase::Chord, PAD, REVERB),
        Case::new("chorus-pad", Phrase::Chord, PAD, CHORUS),
        Case::new("flanger-lead", Phrase::Mono, LEAD,
            &[(42, 1.), (75, 2.), (76, 1.), (77, 0.3), (78, 0.02), (79, 65.), (80, 40.)]),
        Case::new("phaser-keys", Phrase::Chord, CHORD, &[(42, 5.)]),
        Case::new("eq-keys", Phrase::Chord, CHORD, EQ),
        Case::new("limiter-chords", Phrase::Chord, CHORD, &[(108, 1.), (22, 6.)]),
        Case::new("chain-pad", Phrase::Chord, PAD,
            &[(42, 1.), (80, 30.), (43, 3.), (54, 7.), (55, 30.), (56, 20.), (61, 1.),
              (44, 6.), (71, 2.5), (72, 5000.), (74, 20.), (34, 1.), (36, -3.), (38, 2.), (41, -2.)]),
        Case::new("chain-lead", Phrase::Mono, LEAD,
            &[(42, 4.), (32, 9.), (33, 40.), (43, 5.), (102, 30.),
              (44, 2.), (123, -12.), (124, 3.), (128, 2.), (34, 1.), (36, -3.), (41, 2.), (108, 1.)]),
        Case::new("release-tail", Phrase::Tail, PAD, REVERB),
        Case::new("idle-pad", Phrase::Idle, PAD, &[]),
    ];
    // 操作されることの多い連続量を、個別に4秒周期で往復させる。
    for (name, phrase, settings, extra, index, low, high) in [
        ("auto-cutoff", Phrase::Chord, CHORD, &[][..], 10, 1200., 4800.),
        ("auto-resonance", Phrase::Chord, CHORD, &[], 11, 0.7, 2.5),
        ("auto-cutoff-amount", Phrase::Chord, CHORD, &[], 19, 0., 2400.),
        ("auto-drive", Phrase::Chord, CHORD, SOFT, 32, 3., 15.),
        ("auto-mix", Phrase::Chord, CHORD, SOFT, 33, 10., 50.),
        ("auto-pan", Phrase::Chord, CHORD, &[], 21, -40., 40.),
        ("auto-volume", Phrase::Chord, CHORD, &[], 13, -12., 0.),
        ("auto-width", Phrase::Chord, CHORD, &[], 20, 20., 80.),
        ("auto-phase-mod", Phrase::Arpeggio, BELL, &[], 29, 10., 40.),
        ("auto-pitch-bend", Phrase::Legato, LEAD, &[], parameters::PITCH_BEND_INDEX, -1., 1.),
        ("auto-eq-frequency", Phrase::Chord, CHORD, EQ, 37, 500., 2500.),
        ("auto-delay-feedback", Phrase::Mono, LEAD, DELAY, 52, 20., 55.),
        ("auto-reverb-mix", Phrase::Chord, PAD, REVERB, 66, 10., 40.),
        ("auto-chorus-depth", Phrase::Chord, PAD, CHORUS, 76, 1., 4.),
        ("auto-phaser-center", Phrase::Chord, CHORD, &[(42, 5.)], 93, 500., 2000.),
        ("auto-compressor-threshold", Phrase::Chord, CHORD, &[(42, 2.)], 109, -30., -12.),
    ] {
        let mut case = Case::new(name, phrase, settings, extra);
        case.automation = Some(Automation { index, low, high, period_seconds: 4. });
        cases.push(case);
    }
    cases
}

struct Event {
    at: usize,
    note: Note,
    velocity: f32,
    on: bool,
}
struct Scenario {
    parameters: [f64; parameters::COUNT],
    events: Vec<Event>,
    // ホスト相当の変更点から計算した現在値。計測前に用意する。
    automation: Vec<f64>,
    parameter: Option<usize>,
}
fn scenario(case: &Case, rate: usize, block: usize, length: usize) -> Scenario {
    let mut plain = BASE;
    for &(i, v) in case.settings.iter().chain(case.extra) {
        plain[i] = v;
    }
    if let Some(a) = case.automation.filter(|a| a.index < parameters::COUNT) {
        plain[a.index] = a.low;
    }
    assert!(
        plain
            .iter()
            .enumerate()
            .all(|(i, &v)| parameters::valid_plain(i, v))
    );
    let parameters = std::array::from_fn(|i| parameters::to_normalized(i, plain[i]));
    let mut events = Vec::new();
    let (step_ms, gate_ms) = match case.phrase {
        Phrase::Mono => (500, 400),
        Phrase::Legato => (500, 550),
        Phrase::Bass => (250, 190),
        Phrase::Arpeggio => (250, 160),
        Phrase::Chord | Phrase::Tail => (2000, 1750),
        Phrase::Idle => (2000, 0),
    };
    if !matches!(case.phrase, Phrase::Idle) {
        for n in 0.. {
            let at = n * step_ms * rate / 1000;
            if at >= length || (matches!(case.phrase, Phrase::Tail) && n > 0) {
                break;
            }
            let pitches: &[i16] = match case.phrase {
                Phrase::Mono | Phrase::Legato => {
                    &[60, 64, 67, 65, 62, 60, 67, 64][n % 8..n % 8 + 1]
                }
                Phrase::Bass => &[36, 36, 43, 39, 41, 41, 43, 39][n % 8..n % 8 + 1],
                Phrase::Arpeggio => &[60, 64, 67, 72, 67, 64, 62, 67][n % 8..n % 8 + 1],
                Phrase::Tail => &[48, 55, 60, 64],
                Phrase::Chord if n % 2 == 0 => &[48, 55, 60, 64],
                Phrase::Chord => &[53, 60, 65, 69],
                Phrase::Idle => unreachable!(),
            };
            for (i, &pitch) in pitches.iter().enumerate() {
                let note = Note {
                    id: (n * 4 + i + 1) as i32,
                    channel: 0,
                    pitch,
                };
                let velocity = [0.8, 0.65, 0.75, 0.7][n % 4];
                events.push(Event {
                    at,
                    note,
                    velocity,
                    on: true,
                });
                events.push(Event {
                    at: at + gate_ms * rate / 1000,
                    note,
                    velocity,
                    on: false,
                });
            }
        }
    }
    events.sort_by_key(|e| (e.at, e.on)); // 同時刻はNote Off→Note On。
    let mut automation = Vec::new();
    if let Some(a) = case.automation {
        automation.reserve(length);
        let point = |n: usize| {
            let phase = (n as f64 / rate as f64 / a.period_seconds).fract();
            let triangle = 1.0 - (2.0 * phase - 1.0).abs();
            parameters::to_normalized(a.index, a.low + (a.high - a.low) * triangle)
        };
        for start in (0..length).step_by(block) {
            let count = block.min(length - start);
            let before = point(start);
            let after = point(start + count);
            let value = |i: usize| {
                if i == count {
                    after
                } else if parameters::holds_between_points(a.index) {
                    before
                } else {
                    before + (after - before) * i as f64 / count as f64
                }
            };
            // 各ブロック末尾の既知点まで補間する。
            for i in 1..=count {
                automation.push(value(i));
            }
        }
    }
    Scenario {
        parameters,
        events,
        automation,
        parameter: case.automation.map(|a| a.index),
    }
}

struct PlaybackState {
    synth: synth::Synthesizer,
    parameters: [f64; parameters::COUNT],
    event: usize,
    position: usize,
}
struct Playback {
    state: PlaybackState,
}
impl Playback {
    fn new(scenario: &Scenario, rate: usize) -> Self {
        let mut synth = synth::Synthesizer::new();
        assert!(synth.set_sample_rate(rate as f64));
        // 比較対象の旧Rust APIだけ型を適応する。音色・変更点は共通。
        #[cfg(owned_parameter_api)]
        synth.set_parameters(scenario.parameters);
        #[cfg(not(owned_parameter_api))]
        synth.set_parameters(&scenario.parameters);
        if scenario.parameter == Some(parameters::PITCH_BEND_INDEX) {
            synth.set_pitch_bend(scenario.automation[0]);
        }
        Self {
            state: PlaybackState {
                synth,
                parameters: scenario.parameters,
                event: 0,
                position: 0,
            },
        }
    }
    fn generate_samples(&mut self, scenario: &Scenario, block: usize, output: &mut [[f64; 2]]) {
        let s = &mut self.state;
        for chunk in output.chunks_mut(block) {
            let _float_mode = float_mode::FloatMode::flush_subnormals();
            s.synth.set_tempo(120.0);
            if scenario.parameter.is_none() {
                let mut at = 0;
                while at < chunk.len() {
                    while let Some(event) = scenario.events.get(s.event) {
                        if event.at != s.position {
                            break;
                        }
                        if event.on {
                            assert!(s.synth.note_on(event.note, 0., event.velocity));
                        } else {
                            s.synth.note_off(event.note);
                        }
                        s.event += 1;
                    }
                    let end = scenario
                        .events
                        .get(s.event)
                        .map_or(s.position + chunk.len() - at, |e| {
                            e.at.min(s.position + chunk.len() - at)
                        });
                    let count = end - s.position;
                    black_box(&mut s.synth).generate_samples(&mut chunk[at..at + count]);
                    at += count;
                    s.position += count;
                }
            } else {
                for sample in chunk {
                    if let Some(index) = scenario.parameter {
                        let value = scenario.automation[s.position];
                        if index == parameters::PITCH_BEND_INDEX {
                            s.synth.set_pitch_bend(black_box(value));
                        } else {
                            s.parameters[index] = value;
                            #[cfg(owned_parameter_api)]
                            s.synth.set_parameters(black_box(s.parameters));
                            #[cfg(not(owned_parameter_api))]
                            {
                                let mut candidates = [0; parameters::COUNT.div_ceil(64)];
                                candidates[index / 64] = 1 << (index % 64);
                                s.synth
                                    .set_automated_parameters(black_box(&s.parameters), candidates);
                            }
                        }
                    }
                    while let Some(event) = scenario.events.get(s.event) {
                        if event.at != s.position {
                            break;
                        }
                        if event.on {
                            assert!(s.synth.note_on(event.note, 0., event.velocity));
                        } else {
                            s.synth.note_off(event.note);
                        }
                        s.event += 1;
                    }
                    *sample = black_box(&mut s.synth).generate_sample();
                    s.position += 1;
                }
            }
        }
        black_box(output);
    }
}

fn fingerprint(samples: &[[f64; 2]]) -> (u64, f64, f64) {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut peak = 0.0_f64;
    let mut sum = 0.;
    for &x in samples.iter().flatten() {
        assert!(x.is_finite(), "nonfinite audio");
        hash = (hash ^ x.to_bits()).wrapping_mul(0x100000001b3);
        peak = peak.max(x.abs());
        sum += x * x;
    }
    (hash, peak, (sum / (samples.len() * 2) as f64).sqrt())
}

#[cfg(test)]
mod suite_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn cases_have_unique_names_valid_settings_and_all_effects() {
        let mut names = HashSet::new();
        let mut effects = HashSet::new();
        let mut three_slots = false;
        for case in cases() {
            assert!(names.insert(case.name));
            let scene = scenario(&case, 48000, 256, 48000);
            let slots = [42, 43, 44].map(|i| parameters::to_plain(i, scene.parameters[i]) as u8);
            effects.extend(slots);
            three_slots |= slots.iter().all(|&kind| kind != 0);
            if let Some(a) = case.automation {
                assert!(parameters::valid_plain(a.index, a.low));
                assert!(parameters::valid_plain(a.index, a.high));
                assert_eq!(scene.automation.len(), 48000);
                assert!(
                    scene
                        .automation
                        .iter()
                        .all(|x| x.is_finite() && (0.0..=1.0).contains(x))
                );
            }
        }
        assert_eq!(effects, (0..=6).collect());
        assert!(three_slots);
    }

    #[test]
    fn legato_overlaps_and_tail_has_no_new_notes_during_measurement() {
        let cases = cases();
        let legato = cases.iter().find(|c| c.name == "portamento-lead").unwrap();
        let scene = scenario(legato, 48000, 256, 6 * 48000);
        assert_eq!(parameters::to_plain(8, scene.parameters[8]), 1.0);
        assert!(
            (parameters::to_plain(
                parameters::PORTAMENTO,
                scene.parameters[parameters::PORTAMENTO]
            ) - 120.0)
                .abs()
                < 1e-9
        );
        assert!(scene.events[0].on && scene.events[1].on && !scene.events[2].on);
        assert_eq!(scene.events[1].at, 24000);
        assert_eq!(scene.events[2].at, 26400);
        let tail = cases.iter().find(|c| c.name == "release-tail").unwrap();
        let scene = scenario(tail, 48000, 256, 6 * 48000);
        assert_eq!(scene.events.iter().filter(|e| e.on).count(), 4);
        assert!(scene.events.iter().all(|e| e.at < 2 * 48000));
    }

    #[test]
    fn held_cutoff_and_continuous_bend_follow_block_points() {
        let cases = cases();
        let cutoff = cases.iter().find(|c| c.name == "auto-cutoff").unwrap();
        let scene = scenario(cutoff, 48000, 256, 512);
        let low = parameters::to_normalized(10, 1200.0);
        assert!(scene.automation[..255].iter().all(|&v| v == low));
        assert!(scene.automation[255] > low);
        assert!(
            scene.automation[256..511]
                .iter()
                .all(|&v| v == scene.automation[255])
        );
        let bend = cases.iter().find(|c| c.name == "auto-pitch-bend").unwrap();
        let scene = scenario(bend, 48000, 256, 512);
        assert!(scene.automation.windows(2).all(|p| p[0] < p[1]));
        let mut playback = Playback::new(&scene, 48000);
        playback.generate_samples(&scene, 256, &mut [[0.0; 2]; 512]);
        assert_eq!(playback.state.synth.pitch_bend(), scene.automation[511]);
        assert_eq!(
            parameters::to_plain(
                parameters::PITCH,
                playback.state.synth.parameters()[parameters::PITCH]
            ),
            200.0
        );
    }

    #[test]
    fn splitting_playback_preserves_event_and_automation_positions() {
        let cases = cases();
        for name in [
            "portamento-lead",
            "chain-pad",
            "auto-cutoff",
            "auto-pitch-bend",
        ] {
            let case = cases.iter().find(|c| c.name == name).unwrap();
            let scene = scenario(case, 44100, 127, 44100);
            let mut full = Playback::new(&scene, 44100);
            let mut split = Playback::new(&scene, 44100);
            let mut expected = vec![[0.0; 2]; 44100];
            let mut actual = expected.clone();
            full.generate_samples(&scene, 127, &mut expected);
            split.generate_samples(&scene, 127, &mut actual[..22051]);
            split.generate_samples(&scene, 127, &mut actual[22051..]);
            assert_eq!(full.state.position, split.state.position);
            assert_eq!(full.state.event, split.state.event);
            let energy: f64 = expected.iter().flatten().map(|x| x * x).sum();
            let error: f64 = actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).powi(2))
                .sum();
            assert!(
                energy > 0.0 && error < energy * 1e-8,
                "{name}: {error}/{energy}"
            );
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let cases = cases();
    if args.get(1).is_some_and(|a| a == "--list") {
        for case in &cases {
            println!("{}", case.name);
        }
        return;
    }
    assert_eq!(
        args.len(),
        6,
        "usage: standard RATE BUFFER AUDIO_SECONDS CASE BURN_IN_MS"
    );
    let rate: usize = args[1].parse().unwrap();
    let block: usize = args[2].parse().unwrap();
    let seconds: usize = args[3].parse().unwrap();
    let burn_in_ms: u64 = args[5].parse().unwrap();
    assert!(
        [44100, 48000, 96000].contains(&rate)
            && (1..=2048).contains(&block)
            && (4..=60).contains(&seconds)
            && burn_in_ms >= 10
    );
    let case = cases
        .iter()
        .find(|c| c.name == args[4])
        .expect("unknown case");
    let warmup = 2 * rate;
    let length = seconds * rate;
    let scenario = scenario(case, rate, block, warmup + length);
    let mut warm = vec![[0.; 2]; warmup];
    let mut output = vec![[0.; 2]; length];
    // 演奏全体を最低1周処理して、測定区間で使う処理経路もウォームアップする。
    let burn_start = Instant::now();
    let mut burn_in_cycles = 0;
    let mut playback = loop {
        let mut playback = Playback::new(&scenario, rate);
        playback.generate_samples(&scenario, block, &mut warm);
        // 最終回の音源準備もburn-inに含め、常に同じ演奏位置から計測する。
        if burn_in_cycles > 0 && burn_start.elapsed() >= Duration::from_millis(burn_in_ms) {
            break playback;
        }
        playback.generate_samples(&scenario, block, &mut output);
        burn_in_cycles += 1;
    };
    let burn_in_ns = burn_start.elapsed().as_nanos();
    let start = Instant::now();
    playback.generate_samples(&scenario, block, &mut output);
    let measurement_ns = start.elapsed().as_nanos();
    let ns = measurement_ns as f64 / length as f64;
    let (hash, peak, rms) = fingerprint(&output);
    assert_eq!(
        matches!(case.phrase, Phrase::Idle),
        peak == 0.0,
        "unexpected silence/nonzero"
    );
    println!("case,ns,burn_in_ns,burn_in_cycles,measurement_ns,fingerprint,peak,rms");
    println!(
        "{},{ns:.6},{burn_in_ns},{burn_in_cycles},{measurement_ns},{hash:016x},{peak:.12},{rms:.12}",
        case.name
    );
}

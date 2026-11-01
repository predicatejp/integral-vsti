// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! 通常の演奏・音色・オートメーションを固定した性能回帰スイート。
#![allow(dead_code)] // ABI・保存処理など、計測対象以外の既存定義も読み込む。
#[path = "../src/abi.rs"]
mod abi;
#[path = "../src/distortion.rs"]
mod distortion;
#[path = "../src/envelope.rs"]
mod envelope;
#[path = "../src/filter.rs"]
mod filter;
#[path = "../src/float_mode.rs"]
mod float_mode;
#[path = "../src/oscillator.rs"]
mod oscillator;
#[path = "../src/oversampling.rs"]
mod oversampling;
#[path = "../src/parameters.rs"]
mod parameters;
#[path = "../src/random.rs"]
mod random;
#[path = "../src/synth.rs"]
mod synth;
#[path = "../src/voice.rs"]
mod voice;
#[path = "../src/volume.rs"]
mod volume;

use std::{hint::black_box, time::Instant};
use voice::Note;

// 設定はUIの初期値から独立させ、新パラメーターの追加時には明示的に見直す。
const BASE: [f64; parameters::COUNT] = [
    0.,
    10.,
    0.,
    120.,
    0.7,
    100.,
    1.,
    0.,
    1.,
    0.,
    3600.,
    1.,
    1.,
    -6.,
    10.,
    0.,
    100.,
    0.7,
    100.,
    0.,
    0.,
    0.,
    0.,
    0.,
    0.,
    f64::NEG_INFINITY,
    0.,
    0.,
    f64::NEG_INFINITY,
    0.,
    0.,
    0.,
    0.,
    100.,
    0.,
    0.,
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

fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "sine-mono",
            phrase: Phrase::Mono,
            settings: &[],
            extra: &[],
            automation: None,
        },
        Case {
            name: "saw-bass",
            phrase: Phrase::Bass,
            settings: BASS,
            extra: &[],
            automation: None,
        },
        Case {
            name: "triangle-pluck",
            phrase: Phrase::Arpeggio,
            settings: &[
                (0, 1.),
                (1, 2.),
                (3, 250.),
                (4, 0.),
                (5, 180.),
                (8, 4.),
                (9, 1.),
            ],
            extra: &[],
            automation: None,
        },
        Case {
            name: "square-lead",
            phrase: Phrase::Mono,
            settings: LEAD,
            extra: &[],
            automation: None,
        },
        Case {
            name: "saw-chords",
            phrase: Phrase::Chord,
            settings: CHORD,
            extra: &[],
            automation: None,
        },
        Case {
            name: "slow-pad",
            phrase: Phrase::Chord,
            settings: PAD,
            extra: &[],
            automation: None,
        },
        Case {
            name: "three-osc-keys",
            phrase: Phrase::Chord,
            settings: &[
                (0, 3.),
                (5, 400.),
                (8, 8.),
                (9, 1.),
                (22, -6.),
                (23, 1.),
                (24, -1200.),
                (25, -12.),
                (28, -18.),
            ],
            extra: &[],
            automation: None,
        },
        Case {
            name: "phase-mod-bell",
            phrase: Phrase::Arpeggio,
            settings: BELL,
            extra: &[],
            automation: None,
        },
        Case {
            name: "soft-x1-off",
            phrase: Phrase::Bass,
            settings: BASS,
            extra: &[(31, 1.), (32, 9.), (33, 40.)],
            automation: None,
        },
        Case {
            name: "soft-x2-first",
            phrase: Phrase::Bass,
            settings: BASS,
            extra: &[(31, 1.), (32, 9.), (33, 40.), (34, 1.), (35, 1.)],
            automation: None,
        },
        Case {
            name: "soft-x4-second",
            phrase: Phrase::Bass,
            settings: BASS,
            extra: &[(31, 1.), (32, 9.), (33, 40.), (34, 2.), (35, 2.)],
            automation: None,
        },
        Case {
            name: "hard-x2-second",
            phrase: Phrase::Mono,
            settings: LEAD,
            extra: &[(31, 2.), (32, 6.), (33, 25.), (34, 1.), (35, 2.)],
            automation: None,
        },
        Case {
            name: "idle-pad",
            phrase: Phrase::Idle,
            settings: PAD,
            extra: &[],
            automation: None,
        },
    ];
    for (name, index, low, high, seconds) in [
        ("auto-cutoff", 10, 1200., 4800., 4.),
        ("auto-resonance", 11, 0.7, 2.5, 4.),
        ("auto-drive", 32, 3., 15., 4.),
        ("auto-mix", 33, 10., 50., 4.),
        ("auto-pan", 21, -40., 40., 4.),
        ("auto-volume", 13, -12., 0., 4.),
        ("auto-width", 20, 20., 80., 4.),
        ("auto-phase-mod", 29, 10., 40., 4.),
    ] {
        let distortion = index == 32 || index == 33;
        cases.push(Case {
            name,
            phrase: if index == 29 {
                Phrase::Arpeggio
            } else {
                Phrase::Chord
            },
            settings: if index == 29 { BELL } else { CHORD },
            extra: if distortion {
                &[(31, 1.), (32, 9.), (33, 40.), (34, 1.), (35, 1.)]
            } else {
                &[]
            },
            automation: Some(Automation {
                index,
                low,
                high,
                period_seconds: seconds,
            }),
        });
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
    // ホスト相当の変更点から計算した現在値と次境界値。計測前に用意する。
    automation: Vec<[f64; 2]>,
    parameter: Option<usize>,
}
fn scenario(case: &Case, rate: usize, block: usize, length: usize) -> Scenario {
    let mut plain = BASE;
    for &(i, v) in case.settings.iter().chain(case.extra) {
        plain[i] = v;
    }
    if let Some(a) = case.automation {
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
        Phrase::Bass => (250, 190),
        Phrase::Arpeggio => (250, 160),
        Phrase::Chord => (2000, 1750),
        Phrase::Idle => (2000, 0),
    };
    if !matches!(case.phrase, Phrase::Idle) {
        for n in 0.. {
            let at = n * step_ms * rate / 1000;
            if at >= length {
                break;
            }
            let pitches: &[i16] = match case.phrase {
                Phrase::Mono => &[60, 64, 67, 65, 62, 60, 67, 64][n % 8..n % 8 + 1],
                Phrase::Bass => &[36, 36, 43, 39, 41, 41, 43, 39][n % 8..n % 8 + 1],
                Phrase::Arpeggio => &[60, 64, 67, 72, 67, 64, 62, 67][n % 8..n % 8 + 1],
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
            // 各ブロック末尾に1点。次境界もそのブロックの既知点までで評価する。
            for i in 1..=count {
                automation.push([value(i), value((i + 1).min(count))]);
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
        synth.set_sample_rate(rate as f64);
        synth.set_parameters(scenario.parameters);
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
            for sample in chunk {
                if let Some(index) = scenario.parameter {
                    s.parameters[index] = scenario.automation[s.position][0];
                    s.synth.set_parameters(black_box(s.parameters));
                    if index == 32 {
                        s.synth
                            .set_distortion_gain_end(scenario.automation[s.position][1]);
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
        "usage: standard RATE BLOCK ROUNDS SECONDS CASE_PREFIX"
    );
    let rate: usize = args[1].parse().unwrap();
    let block: usize = args[2].parse().unwrap();
    let rounds: usize = args[3].parse().unwrap();
    let seconds: usize = args[4].parse().unwrap();
    assert!(
        [44100, 48000, 96000].contains(&rate)
            && (1..=2048).contains(&block)
            && (3..=31).contains(&rounds)
            && rounds % 2 == 1
            && (4..=60).contains(&seconds)
    );
    let warmup = 2 * rate;
    let length = seconds * rate;
    println!("case,min_ns,median_ns,max_ns,fingerprint,peak,rms");
    let mut matched = 0;
    for case in cases.iter().filter(|c| c.name.starts_with(&args[5])) {
        matched += 1;
        let scenario = scenario(case, rate, block, warmup + length);
        let mut warm = vec![[0.; 2]; warmup];
        let mut output = vec![[0.; 2]; length];
        let mut times = Vec::with_capacity(rounds);
        let mut reference = None;
        for _ in 0..rounds {
            let mut playback = Playback::new(&scenario, rate);
            playback.generate_samples(&scenario, block, &mut warm);
            let start = Instant::now();
            playback.generate_samples(&scenario, block, &mut output);
            times.push(start.elapsed().as_secs_f64() * 1e9 / length as f64);
            let check = fingerprint(&output);
            assert_eq!(
                matches!(case.phrase, Phrase::Idle),
                check.1 == 0.0,
                "unexpected silence/nonzero"
            );
            if let Some(old) = reference {
                assert_eq!(check, old, "non-reproducible audio");
            }
            reference = Some(check);
        }
        times.sort_by(f64::total_cmp);
        let (hash, peak, rms) = reference.unwrap();
        println!(
            "{},{:.3},{:.3},{:.3},{hash:016x},{peak:.12},{rms:.12}",
            case.name,
            times[0],
            times[rounds / 2],
            times[rounds - 1]
        );
    }
    assert!(matched > 0, "no matching cases");
}

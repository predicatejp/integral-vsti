// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! テーブル参照の速度・誤差実験。本体にはリンクしない。
#![allow(dead_code)]
#[path = "../src/abi.rs"]
mod abi;
#[path = "../src/envelope.rs"]
mod envelope;
#[path = "../src/filter.rs"]
mod filter;
#[path = "../src/oscillator.rs"]
mod oscillator;
#[path = "../src/parameters.rs"]
mod parameters;
#[path = "../src/synth.rs"]
mod synth;
#[path = "../src/voice.rs"]
mod voice;

use std::{hint::black_box, io::Write, time::Instant};
use synth::Synthesizer;
use voice::Note;

fn note_formula(note: usize, cents: f64) -> f64 {
    440.0 * 2.0_f64.powf((note as f64 - 69.0 + cents / 100.0) / 12.0)
}

fn note_lookup(note: usize, cents: f64, table: &[f64; 128]) -> f64 {
    if cents == 0.0 {
        table[note]
    } else {
        table[note] * 2.0_f64.powf(cents / 1200.0)
    }
}

fn quarter_sine(phase: f64, table: &[f64]) -> f64 {
    // 発振器の位相[0, 1)を、対称性を使って第1象限へ折り返す。
    let quadrant_position = phase * 4.0;
    let quadrant = quadrant_position as usize;
    let fraction = quadrant_position - quadrant as f64;
    let folded = if quadrant & 1 == 0 {
        fraction
    } else {
        1.0 - fraction
    };
    let position = folded * (table.len() - 1) as f64;
    // 頂点も最後の区間のfraction=1として扱い、範囲外参照を防ぐ。
    let index = (position as usize).min(table.len() - 2);
    let value = table[index] + (table[index + 1] - table[index]) * (position - index as f64);
    if quadrant & 2 == 0 { value } else { -value }
}

fn batch(iterations: usize, operation: &mut impl FnMut(usize) -> f64) -> f64 {
    let start = Instant::now();
    for i in 0..iterations {
        black_box(operation(i));
    }
    start.elapsed().as_secs_f64()
}

fn measure(name: &str, mut operation: impl FnMut(usize) -> f64) {
    batch(4096, &mut operation);
    let mut iterations = 1024;
    while batch(iterations, &mut operation) < 0.02 {
        iterations *= 2;
    }
    let mut times = [0.0; 7];
    for time in &mut times {
        *time = batch(iterations, &mut operation) * 1e9 / iterations as f64;
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{name},{iterations},{:.3},{:.3},{:.3}",
        times[0], times[3], times[6]
    );
}

fn micro() {
    let notes: [f64; 128] = std::array::from_fn(|n| note_formula(n, 0.0));
    // 先に全候補を構築し、テーブル初期化を計測から除く。
    let tables: Vec<Vec<f64>> = [64, 256, 1024, 4096]
        .into_iter()
        .map(|n| {
            (0..=n)
                .map(|i| (std::f64::consts::FRAC_PI_2 * i as f64 / n as f64).sin())
                .collect()
        })
        .collect();
    println!("case,iterations,min_ns,median_ns,max_ns");
    measure("baseline", |i| black_box(i as f64));
    for mode in ["zero", "tuned", "mixed"] {
        let cents = |i: usize| match mode {
            "zero" => 0.0,
            "tuned" => (i % 2001) as f64 * 0.1 - 100.0,
            _ => {
                if i % 8 == 0 {
                    17.5
                } else {
                    0.0
                }
            }
        };
        measure(&format!("note/formula/{mode}"), |i| {
            note_formula(black_box(i % 128), black_box(cents(i)))
        });
        measure(&format!("note/table/{mode}"), |i| {
            note_lookup(black_box(i % 128), black_box(cents(i)), black_box(&notes))
        });
    }
    // 均一な位相巡回と、440Hzの連続した位相更新を分ける。
    for sequential in [false, true] {
        let mode = if sequential { "440hz" } else { "spread" };
        let mut phase = 0.0_f64;
        measure(&format!("sine/direct/{mode}"), |i| {
            phase = if sequential {
                (phase + 440.0 / 48000.0).fract()
            } else {
                ((i * 4051) % 65536) as f64 / 65536.0
            };
            (std::f64::consts::TAU * black_box(phase)).sin()
        });
        for table in &tables {
            let mut phase = 0.0_f64;
            measure(&format!("sine/table{}/{mode}", table.len() - 1), |i| {
                phase = if sequential {
                    (phase + 440.0 / 48000.0).fract()
                } else {
                    ((i * 4051) % 65536) as f64 / 65536.0
                };
                quarter_sine(black_box(phase), black_box(table))
            });
        }
    }
    eprintln!("accuracy: sine intervals,bytes,max_abs,rms_error,relative_rms_gain_error");
    // テーブル格子と一致しない分母で全周期を走査し、4象限とその直近も確認。
    for table in &tables {
        let count = 1_048_613;
        let (mut max, mut error2, mut signal2) = (0.0_f64, 0.0, 0.0);
        for i in 0..count {
            let phase = i as f64 / count as f64;
            let actual = quarter_sine(phase, table);
            let error = actual - (std::f64::consts::TAU * phase).sin();
            max = max.max(error.abs());
            error2 += error * error;
            signal2 += actual * actual;
        }
        for phase in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            for p in [phase.next_down(), phase, phase.next_up()] {
                if (0.0..1.0).contains(&p) {
                    let actual = quarter_sine(p, table);
                    assert!(actual.is_finite() && actual.abs() <= 1.0);
                    assert!((actual - (std::f64::consts::TAU * p).sin()).abs() < 1e-12);
                }
            }
        }
        let rms = (error2 / count as f64).sqrt();
        let gain_error = (signal2 / count as f64).sqrt() * std::f64::consts::SQRT_2 - 1.0;
        eprintln!(
            "sine,{},{},{max:.12e},{rms:.12e},{gain_error:.12e}",
            table.len() - 1,
            table.len() * 8
        );
    }
    let (mut relative, mut cent_error) = (0.0_f64, 0.0_f64);
    for n in 0..128 {
        assert_eq!(note_lookup(n, 0.0, &notes), note_formula(n, 0.0));
        for c in -1000..=1000 {
            let cents = c as f64 * 0.1;
            let ratio = note_lookup(n, cents, &notes) / note_formula(n, cents);
            relative = relative.max((ratio - 1.0).abs());
            cent_error = cent_error.max((1200.0 * ratio.log2()).abs());
        }
    }
    eprintln!("note,128,1024,max_relative={relative:.12e},max_cents={cent_error:.12e}");
}

fn make_synth(rate: f64, polyphony: usize, unison: usize) -> Synthesizer {
    let mut synth = Synthesizer::new();
    synth.set_sample_rate(rate);
    let mut values = parameters::defaults();
    values[6] = parameters::to_normalized(6, unison as f64);
    values[7] = parameters::to_normalized(7, 30.0);
    values[8] = parameters::to_normalized(8, polyphony as f64);
    values[9] = parameters::to_normalized(9, 2.0);
    synth.set_parameters(values);
    for i in 0..polyphony {
        assert!(synth.note_on(
            Note {
                id: i as i32,
                channel: 0,
                pitch: 48 + i as i16
            },
            0.0,
            1.0
        ));
    }
    for _ in 0..rate as usize {
        synth.generate_sample();
    }
    synth
}

fn integrated(rate: f64) {
    println!("case,iterations,min_ns,median_ns,max_ns");
    for (polyphony, unison) in [(1, 1), (1, 7), (32, 7)] {
        let mut synth = make_synth(rate, polyphony, unison);
        measure(&format!("synth/p{polyphony}/u{unison}"), |_| {
            black_box(&mut synth).generate_sample()
        });
    }
    let mut synth = make_synth(rate, 32, 7);
    for tuned in [false, true] {
        measure(
            if tuned {
                "note_on/tuned"
            } else {
                "note_on/zero"
            },
            |i| {
                f64::from(black_box(&mut synth).note_on(
                    black_box(Note {
                        id: i as i32,
                        channel: 0,
                        pitch: (i % 128) as i16,
                    }),
                    black_box(if tuned {
                        (i % 2001) as f32 * 0.1 - 100.0
                    } else {
                        0.0
                    }),
                    1.0,
                ))
            },
        );
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map_or("micro", String::as_str) {
        "micro" => micro(),
        "synth" => integrated(args[2].parse().unwrap()),
        "tables" => {
            let intervals: usize = args[2].parse().unwrap();
            let notes: Vec<_> = (0..128).map(|n| note_formula(n, 0.0)).collect();
            let sine: Vec<_> = (0..=intervals)
                .map(|n| (std::f64::consts::FRAC_PI_2 * n as f64 / intervals as f64).sin())
                .collect();
            println!("pub const NOTES: [f64; 128] = {notes:?};");
            println!("pub const SINE: [f64; {}] = {sine:?};", intervals + 1);
        }
        "audio" => {
            let mut out = std::io::BufWriter::new(std::io::stdout().lock());
            for rate in [48000.0, 96000.0] {
                for (polyphony, unison) in [(1, 1), (1, 7), (32, 7)] {
                    let mut synth = make_synth(rate, polyphony, unison);
                    for _ in 0..8192 {
                        let sample = synth.generate_sample();
                        assert!(sample.is_finite());
                        out.write_all(&sample.to_le_bytes()).unwrap();
                    }
                }
            }
        }
        _ => panic!("use micro, synth <rate>, tables <intervals>, or audio"),
    }
}

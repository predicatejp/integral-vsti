// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! 最適化前後へ同じ入力系列を与えるオフライン検証。音源本体にはリンクしない。
#![allow(dead_code)]
#[path = "src/abi.rs"]
mod abi;
#[path = "src/envelope.rs"]
mod envelope;
#[path = "src/filter.rs"]
mod filter;
#[path = "float_mode.rs"]
mod float_mode;
#[path = "src/oscillator.rs"]
mod oscillator;
#[path = "src/parameters.rs"]
mod parameters;
#[path = "src/synth.rs"]
mod synth;
#[path = "src/voice.rs"]
mod voice;
use filter::{Filter, FilterParameters};
#[path = "automation.rs"]
mod automation;
use std::io::Write;
use synth::Synthesizer;
use voice::Note;

fn record(
    out: &mut impl Write,
    manifest: &mut impl Write,
    name: &str,
    count: usize,
    mut sample: impl FnMut(usize) -> f64,
) {
    writeln!(manifest, "{name},{count}").unwrap();
    for n in 0..count {
        out.write_all(&sample(n).to_le_bytes()).unwrap();
    }
}
fn note(id: usize, pitch: i16) -> Note {
    Note {
        id: id as i32,
        channel: 0,
        pitch,
    }
}
fn settings(wave: usize, kind: usize, cut: f64, q: f64, poly: usize, u: usize) -> [f64; 12] {
    let mut p = parameters::defaults();
    for (i, v) in [
        (0, wave as f64),
        (1, 0.0),
        (2, 0.0),
        (3, 0.0),
        (4, 1.0),
        (5, 2.0),
        (6, u as f64),
        (7, 37.5),
        (8, poly as f64),
        (9, kind as f64),
        (10, cut),
        (11, q),
    ] {
        p[i] = parameters::to_normalized(i, v);
    }
    p
}
fn excitation(n: usize, rate: f64) -> f64 {
    0.25 * (std::f64::consts::TAU * 440.0 * n as f64 / rate).sin()
        + 0.25 * (2.0 * (n as f64 * 1703.0 / rate).fract() - 1.0)
}
fn main() {
    let _mode = if cfg!(flush) {
        Some(float_mode::FloatMode::flush_subnormals())
    } else {
        None
    };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut manifest =
        std::io::BufWriter::new(std::fs::File::create(std::env::args().nth(1).unwrap()).unwrap());
    writeln!(manifest, "case,samples").unwrap();
    if std::env::args().nth(2).as_deref() == Some("automation") {
        let path = std::path::PathBuf::from(std::env::args().nth(1).unwrap())
            .with_file_name("parameter-trace.csv");
        let mut trace = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        automation::run(&mut out, &mut manifest, &mut trace);
        return;
    }
    if std::env::args().nth(2).as_deref() == Some("user") {
        for kind in 0..7 {
            for cut in [-2400.0, 0.0, 1200.0, 9600.0] {
                for q in [0.5, std::f64::consts::FRAC_1_SQRT_2, 10.0] {
                    for active in [1, 32] {
                        let mut s = Synthesizer::new();
                        s.set_sample_rate(48000.0);
                        let mut p = settings(3, kind, cut, q, 32, 7);
                        p[7] = 0.25;
                        s.set_parameters(p);
                        for i in 0..active {
                            s.note_on(note(i, 48 + i as i16), 0.0, 1.0);
                        }
                        record(
                            &mut out,
                            &mut manifest,
                            &format!("user/fixed/r48000/f{kind}/c{cut}/q{q}/active{active}"),
                            48000,
                            |_| s.generate_sample(),
                        );
                    }
                }
            }
            for mode in ["long", "cut-step", "q-step", "both-toggle", "notes"] {
                let mut s = Synthesizer::new();
                s.set_sample_rate(48000.0);
                let mut p = settings(3, kind, 1200.0, 10.0, 32, 7);
                p[7] = 0.25;
                s.set_parameters(p);
                for i in 0..32 {
                    s.note_on(note(i, 48 + i as i16), 0.0, 1.0);
                }
                let count = if mode == "long" { 480000 } else { 48000 };
                record(
                    &mut out,
                    &mut manifest,
                    &format!("user/{mode}/r48000/f{kind}/q10/active32"),
                    count,
                    |n| {
                        match mode {
                            "cut-step" => p[10] = if n % 128 < 64 { 0.0 } else { 1.0 },
                            "q-step" => p[11] = if n % 128 < 64 { 0.0 } else { 1.0 },
                            "both-toggle" => {
                                p[10] = (n % 2) as f64;
                                p[11] = 1.0 - p[10];
                            }
                            _ => {}
                        }
                        s.set_parameters(p);
                        if mode == "notes" {
                            if n % 127 == 0 {
                                let i = (n / 127) % 32;
                                s.note_off(note(i, 48 + i as i16));
                            }
                            if n % 193 == 0 {
                                let i = (n / 193) % 32;
                                s.note_on(note(i, 48 + i as i16), 0.0, 1.0);
                            }
                        }
                        s.generate_sample()
                    },
                );
            }
        }
        return;
    }
    for rate in [32000.0, 44100.0, 48000.0, 96000.0] {
        for kind in 0..7 {
            for hz in [1.0, 20.0, 440.0, 5000.0, 20000.0, 100000.0] {
                for q in [0.5, std::f64::consts::FRAC_1_SQRT_2, 2.0, 10.0] {
                    for signal in 0..3 {
                        let mut f = Filter::new();
                        let p = FilterParameters {
                            cutoff_hz: hz,
                            resonance_q: q,
                        };
                        record(
                            &mut out,
                            &mut manifest,
                            &format!("filter/fixed/r{rate}/f{kind}/hz{hz}/q{q}/s{signal}"),
                            1024,
                            |n| {
                                let x = match signal {
                                    0 => {
                                        if n == 0 {
                                            0.5
                                        } else {
                                            0.0
                                        }
                                    }
                                    1 => excitation(n, rate),
                                    _ => {
                                        if n % 48 < 24 {
                                            0.5
                                        } else {
                                            -0.5
                                        }
                                    }
                                };
                                f.process_sample(x, parameters::FILTER_TYPES[kind], &p, rate)
                            },
                        );
                    }
                }
            }
            for mode in [
                "cut-step",
                "cut-ramp",
                "q-step",
                "q-ramp",
                "both-toggle",
                "both-random",
                "type",
                "rate",
                "tail",
            ] {
                let mut f = Filter::new();
                let mut rng = 123456789_u32;
                let count = if mode == "tail" { 65536 } else { 4096 };
                record(
                    &mut out,
                    &mut manifest,
                    &format!("filter/{mode}/r{rate}/f{kind}"),
                    count,
                    |n| {
                        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                        let mut p = FilterParameters {
                            cutoff_hz: 440.0,
                            resonance_q: 10.0,
                        };
                        let mut k = kind;
                        let mut r = rate;
                        match mode {
                            "cut-step" => p.cutoff_hz = if n % 128 < 64 { 20.0 } else { 20000.0 },
                            "cut-ramp" => {
                                p.cutoff_hz = 20.0 * 1000.0_f64.powf((n % 240) as f64 / 239.0)
                            }
                            "q-step" => p.resonance_q = if n % 128 < 64 { 0.5 } else { 10.0 },
                            "q-ramp" => p.resonance_q = 0.5 + 9.5 * (n % 32) as f64 / 31.0,
                            "both-toggle" => {
                                p.cutoff_hz = if n % 2 == 0 { 20.0 } else { 20000.0 };
                                p.resonance_q = if n % 2 == 0 { 0.5 } else { 10.0 };
                            }
                            "both-random" => {
                                p.cutoff_hz = 20.0 + 19980.0 * (rng >> 16) as f64 / 65535.0;
                                p.resonance_q = 0.5 + 9.5 * (rng & 65535) as f64 / 65535.0;
                            }
                            "type" => k = (n / 32) % 7,
                            "rate" => r = if n % 62 < 31 { 32000.0 } else { 96000.0 },
                            _ => {}
                        }
                        let x = if mode == "tail" {
                            if n == 0 { 0.5 } else { 0.0 }
                        } else {
                            excitation(n, rate)
                        };
                        f.process_sample(x, parameters::FILTER_TYPES[k], &p, r)
                    },
                );
            }
        }
    }
    for rate in [44100.0, 48000.0, 96000.0] {
        for kind in 0..7 {
            for wave in 0..4 {
                for cut in [-2400.0, 1200.0, 9600.0] {
                    for q in [0.5, std::f64::consts::FRAC_1_SQRT_2, 10.0] {
                        for u in [1, 7] {
                            let mut s = Synthesizer::new();
                            s.set_sample_rate(rate);
                            s.set_parameters(settings(wave, kind, cut, q, 1, u));
                            s.note_on(note(0, 69), 0.0, 1.0);
                            record(
                                &mut out,
                                &mut manifest,
                                &format!("synth/fixed/r{rate}/f{kind}/w{wave}/c{cut}/q{q}/u{u}"),
                                2048,
                                |_| s.generate_sample(),
                            );
                        }
                    }
                }
                for mode in [
                    "cut-step",
                    "cut-ramp",
                    "q-step",
                    "both-toggle",
                    "detune",
                    "unison",
                    "notes",
                    "all",
                ] {
                    let poly = if mode == "all" { 32 } else { 4 };
                    let mut s = Synthesizer::new();
                    s.set_sample_rate(rate);
                    let mut p = settings(wave, kind, 1200.0, 10.0, poly, 7);
                    s.set_parameters(p);
                    for i in 0..poly {
                        s.note_on(note(i, 48 + i as i16), 0.0, 0.7);
                    }
                    record(
                        &mut out,
                        &mut manifest,
                        &format!("synth/{mode}/r{rate}/f{kind}/w{wave}"),
                        4096,
                        |n| {
                            match mode {
                                "cut-step" => p[10] = if n % 128 < 64 { 0.0 } else { 1.0 },
                                "cut-ramp" => p[10] = (n % 240) as f64 / 239.0,
                                "q-step" => p[11] = if n % 128 < 64 { 0.0 } else { 1.0 },
                                "both-toggle" => {
                                    p[10] = (n % 2) as f64;
                                    p[11] = 1.0 - p[10];
                                }
                                "detune" => {
                                    p[7] = if n % 257 == 0 {
                                        0.0
                                    } else {
                                        (n % 101) as f64 / 100.0 + 0.001 * (n % 7) as f64
                                    }
                                    .min(1.0)
                                }
                                "unison" => p[6] = ((n / 17) % 7) as f64 / 6.0,
                                "all" => {
                                    p[0] = ((n / 113) % 4) as f64 / 3.0;
                                    p[6] = ((n / 17) % 7) as f64 / 6.0;
                                    p[7] = (n % 101) as f64 / 100.0;
                                    p[8] = if n % 512 < 256 { 0.0 } else { 1.0 };
                                    p[9] = ((n / 31) % 7) as f64 / 6.0;
                                    p[10] = (n % 97) as f64 / 96.0;
                                    p[11] = (n % 53) as f64 / 52.0;
                                    for (i, v) in p.iter_mut().enumerate().take(6).skip(1) {
                                        *v = ((n + i * 13) % 71) as f64 / 70.0;
                                    }
                                }
                                _ => {}
                            }
                            s.set_parameters(p);
                            if matches!(mode, "notes" | "all") {
                                if n % 37 == 0 {
                                    s.note_off(note(
                                        (n / 37) % poly,
                                        48 + ((n / 37) % poly) as i16,
                                    ));
                                }
                                if n % 53 == 0 {
                                    s.note_on(
                                        note((n / 53) % poly, 36 + ((n / 53) % 72) as i16),
                                        17.3,
                                        0.8,
                                    );
                                }
                                if n % 1024 == 511 {
                                    s.stop();
                                }
                            }
                            s.generate_sample()
                        },
                    );
                }
            }
        }
    }
    // 小さな周波数倍率誤差の累積と不連続波形のエッジずれを10秒追跡する。
    for wave in 0..4 {
        for kind in [0, 2, 4, 6] {
            let mut s = Synthesizer::new();
            s.set_sample_rate(48000.0);
            s.set_parameters(settings(wave, kind, 0.0, 10.0, 1, 7));
            s.note_on(note(0, 108), 0.0, 1.0);
            record(
                &mut out,
                &mut manifest,
                &format!("synth/long/r48000/f{kind}/w{wave}/n108/q10/u7"),
                480000,
                |_| s.generate_sample(),
            );
        }
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! DSP本体を直接コンパイルする計測専用プログラム。プラグインにはリンクしない。
#![allow(dead_code)] // ABI・保存処理など、計測対象以外の既存定義も読み込む。
#[path = "../src/abi.rs"]
mod abi;
#[path = "../src/distortion.rs"]
mod distortion;
#[path = "../src/effects.rs"]
mod effects;
#[path = "../src/eq.rs"]
mod eq;
#[path = "../src/envelope.rs"]
mod envelope;
#[path = "../src/filter.rs"]
mod filter;
#[path = "../src/float_mode.rs"]
mod float_mode;
#[path = "../src/oscillator.rs"]
mod oscillator;
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

use envelope::{DEFAULT_ENVELOPE, EnvelopeParameters};
use filter::{Filter, FilterParameters, FilterType, Svf};
use oscillator::{Oscillator, OscillatorInput, Waveform};
use std::{hint::black_box, time::Instant};
use synth::Synthesizer;
use voice::{Note, Voice};

static SCOPED_FTZ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn batch<T>(iterations: usize, operation: &mut impl FnMut(usize) -> T) -> f64 {
    let start = Instant::now();
    let scoped = SCOPED_FTZ.load(std::sync::atomic::Ordering::Relaxed);
    for start in (0..iterations).step_by(256) {
        let _mode = scoped.then(float_mode::FloatMode::flush_subnormals);
        for i in start..(start + 256).min(iterations) {
            black_box(operation(i));
        }
    }
    start.elapsed().as_secs_f64()
}

fn measure<T>(name: &str, unit: &str, selection: &str, mut operation: impl FnMut(usize) -> T) {
    if !name.starts_with(selection) {
        return;
    }
    batch(4096, &mut operation); // 初期化・ページフォルト・初回実行を計測から除く。
    let mut iterations = 1024;
    while batch(iterations, &mut operation) < 0.02 {
        iterations *= 2;
    }
    let mut ns = [0.0; 7];
    for value in &mut ns {
        *value = batch(iterations, &mut operation) * 1e9 / iterations as f64;
    }
    ns.sort_by(f64::total_cmp);
    println!(
        "{name},{unit},{iterations},{:.3},{:.3},{:.3}",
        ns[0], ns[3], ns[6]
    );
}

fn input(waveform: Waveform, unison: u32, detune: f64) -> OscillatorInput {
    OscillatorInput {
        phase_mode: oscillator::PhaseMode::Keep,
        waveform,
        unison_voices: unison,
        detune_cents: detune,
        frequency_hz: 440.0,
        phase_modulation: 0.0,
        phase_modulated: false,
        active: true,
        note_generation: 1,
    }
}

fn note(id: i32) -> Note {
    Note {
        id,
        channel: 0,
        pitch: 48 + id as i16,
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let rate: f64 = args
        .get(1)
        .map_or(48_000.0, |s| s.parse().expect("sample rate"));
    assert!(rate.is_finite() && rate > 0.0);
    let selection = args.get(2).map_or("", String::as_str);
    SCOPED_FTZ.store(
        args.get(3).is_some_and(|s| s == "--scoped-ftz"),
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut mxcsr = 0u32;
    // SAFETY: x86_64のSSE制御レジスタを、有効な4バイト領域へ読み出す。
    // 計測専用プロセスの実行スレッドだけを対象とし、プラグイン本体は変更しない。
    unsafe {
        std::arch::asm!("stmxcsr [{p}]", p = in(reg) &mut mxcsr, options(nostack, preserves_flags));
    }
    if args.get(3).is_some_and(|s| s == "--flush-subnormals") {
        mxcsr |= (1 << 15) | (1 << 6); // FTZとDAZ。他の制御ビットは保持する。
        // SAFETY: 読み出した有効な値のFTZ/DAZだけを変更し、同じスレッドへ戻す。
        unsafe {
            std::arch::asm!("ldmxcsr [{p}]", p = in(reg) &mxcsr, options(nostack, readonly, preserves_flags));
        }
    }
    eprintln!(
        "mxcsr=0x{mxcsr:08x} (FTZ={}, DAZ={})",
        mxcsr & (1 << 15) != 0,
        mxcsr & (1 << 6) != 0
    );

    eprintln!(
        "rate={rate}, sizes(bytes): EnvelopeParameters={}, Oscillator={}, Svf={}, Filter={}, Voice={}, Synthesizer={}",
        size_of::<EnvelopeParameters>(),
        size_of::<Oscillator>(),
        size_of::<Svf>(),
        size_of::<Filter>(),
        size_of::<Voice>(),
        size_of::<Synthesizer>()
    );
    println!("case,unit,iterations,min_ns,median_ns,max_ns");
    for clipper in [
        distortion::ClipperType::None,
        distortion::ClipperType::Soft,
        distortion::ClipperType::Hard,
    ] {
        let mut distortion = distortion::Distortion::new();
        let parameters = distortion::DistortionParameters {
            clipper,
            gain: 10.0_f64.powf(12.0 / 20.0),
            mix: 1.0,
        };
        measure(
            &format!("distortion/{clipper:?}"),
            "sample",
            selection,
            |i| {
                let x = (i % 1024) as f64 / 512.0 - 1.0;
                distortion.process_sample(black_box([x, -x]), black_box(&parameters))
            },
        );
    }
    measure("baseline", "operation", selection, |i| black_box(i as f64));
    let envelope = EnvelopeParameters {
        hold_ms: 20.0,
        ..DEFAULT_ENVELOPE
    };
    for (stage, time, off) in [
        ("attack", 5.0, None),
        ("hold", 20.0, None),
        ("decay", 80.0, None),
        ("sustain", 1000.0, None),
        ("release", 1050.0, Some(1000.0)),
    ] {
        measure(&format!("envelope/{stage}"), "sample", selection, |_| {
            envelope::value(
                black_box(&envelope),
                black_box(time),
                black_box(off),
                0.0,
                0.5,
            )
        });
    }
    for waveform in parameters::WAVEFORMS {
        for (unison, detune) in [(1, 0.0), (4, 30.0), (7, 30.0), (7, 0.0)] {
            let input = input(waveform, unison, detune);
            let mut oscillator = Oscillator::new();
            for _ in 0..rate as usize {
                oscillator.generate_sample(&input, rate);
            }
            measure(
                &format!("oscillator/{waveform:?}/u{unison}/d{detune}"),
                "sample",
                selection,
                |_| oscillator.generate_sample(black_box(&input), black_box(rate)),
            );
        }
    }
    let mut modulated = Oscillator::new();
    let mut modulated_input = input(Waveform::Sine, 7, 30.0);
    measure(
        "oscillator/Sine/u7/detune-changing",
        "sample",
        selection,
        |i| {
            modulated_input.detune_cents = 5.0 + (i % 1000) as f64 * 0.075;
            modulated.generate_sample(black_box(&modulated_input), black_box(rate))
        },
    );
    let mut idle = Oscillator::new();
    let mut idle_input = input(Waveform::Sine, 7, 30.0);
    idle_input.active = false;
    idle_input.note_generation = 0;
    measure("oscillator/idle/u7", "sample", selection, |_| {
        idle.generate_sample(black_box(&idle_input), black_box(rate))
    });
    let mut p = FilterParameters {
        cutoff_hz: 880.0,
        resonance_q: std::f64::consts::FRAC_1_SQRT_2,
    };
    let mut svf = Svf::default();
    measure("svf/one-stage", "sample", selection, |i| {
        svf.process_sample(
            black_box(if i % 2 == 0 { 0.5 } else { -0.5 }),
            black_box(&p),
            black_box(rate),
        )
        .low
    });
    for kind in parameters::FILTER_TYPES {
        let mut filter = Filter::new();
        measure(&format!("filter/{kind:?}"), "sample", selection, |i| {
            filter.process_sample(
                [black_box(if i % 2 == 0 { 0.5 } else { -0.5 }); 2],
                black_box(kind),
                black_box(&p),
                black_box(rate),
            )
        });
    }
    let mut filter = Filter::new();
    measure("filter/LowPass2/modulated", "sample", selection, |i| {
        p.cutoff_hz = 880.0 + (i % 128) as f64;
        filter.process_sample(
            [black_box(0.5); 2],
            black_box(FilterType::LowPass2),
            black_box(&p),
            black_box(rate),
        )
    });
    // 完全な無音と、インパルスが十分減衰した状態を分ける。
    p.cutoff_hz = 880.0;
    for tail in [false, true] {
        let mut filter = Filter::new();
        if tail {
            filter.process_sample([1.0; 2], FilterType::LowPass2, &p, rate);
            for _ in 0..(rate * 2.0) as usize {
                filter.process_sample([0.0; 2], FilterType::LowPass2, &p, rate);
            }
        }
        measure(
            if tail {
                "filter/LowPass2/decayed-tail"
            } else {
                "filter/LowPass2/zero-state"
            },
            "sample",
            selection,
            |_| {
                filter.process_sample(
                    [black_box(0.0); 2],
                    black_box(FilterType::LowPass2),
                    black_box(&p),
                    black_box(rate),
                )
            },
        );
    }
    for unison in [1, 7] {
        for kind in [FilterType::None, FilterType::LowPass1, FilterType::LowPass2] {
            let mut voice = Voice::new();
            voice.set_parameters(
                Waveform::Sawtooth,
                DEFAULT_ENVELOPE,
                unison,
                30.0,
                oscillator::PhaseMode::Keep,
            );
            voice.set_filter(kind, 1200.0, p.resonance_q);
            voice.note_on(note(0), 0.0, 1.0);
            for _ in 0..rate as usize {
                voice.generate_sample(rate, &[[1.0; 2]; 7], &[1.0; 2]);
            }
            measure(
                &format!("voice/u{unison}/{kind:?}"),
                "sample",
                selection,
                |_| {
                    black_box(&mut voice).generate_sample(
                        black_box(rate),
                        &[[1.0; 2]; 7],
                        &[1.0; 2],
                    )
                },
            );
        }
    }
    for polyphony in [1, 8, 32] {
        for unison in [1, 7] {
            for mode in [
                "render",
                "unchanged",
                "automation",
                "idle",
                "released",
                "sine",
                "noise",
                "detune",
                "width",
                "width-changing",
                "pan-changing",
            ] {
                let mut synth = Synthesizer::new();
                synth.set_sample_rate(rate);
                let mut values = parameters::defaults();
                values[0] = parameters::to_normalized(
                    0,
                    if matches!(mode, "sine" | "detune") {
                        0.0
                    } else if mode == "noise" {
                        4.0
                    } else {
                        3.0
                    },
                );
                values[6] = parameters::to_normalized(6, unison as f64);
                values[7] = parameters::to_normalized(7, 30.0);
                values[8] = parameters::to_normalized(8, polyphony as f64);
                values[9] = parameters::to_normalized(9, 2.0);
                if matches!(mode, "width" | "width-changing" | "pan-changing") {
                    values[20] = 1.0;
                }
                synth.set_parameters(values);
                if mode != "idle" {
                    for i in 0..polyphony {
                        synth.note_on(note(i), 0.0, 1.0);
                    }
                    for _ in 0..rate as usize {
                        synth.generate_sample();
                    }
                }
                if mode == "released" {
                    for i in 0..polyphony {
                        synth.note_off(note(i));
                    }
                    for _ in 0..(rate * 2.0) as usize {
                        synth.generate_sample();
                    }
                }
                measure(
                    &format!("synth/p{polyphony}/u{unison}/{mode}"),
                    "sample",
                    selection,
                    |i| {
                        if mode == "automation" {
                            values[11] = 0.05 + (i % 256) as f64 / 512.0;
                        }
                        if mode == "detune" {
                            values[7] = (5.0 + (i % 1000) as f64 * 0.075) / 100.0;
                        }
                        if mode == "width-changing" {
                            values[20] = (i % 1000) as f64 / 999.0;
                        }
                        if mode == "pan-changing" {
                            values[21] = (i % 1000) as f64 / 999.0;
                        }
                        if mode != "render" {
                            synth.set_parameters(black_box(values));
                        }
                        black_box(&mut synth).generate_sample()
                    },
                );
            }
        }
    }
    let mut synth = Synthesizer::new();
    let mut values = parameters::defaults();
    values[8] = 1.0;
    synth.set_parameters(values);
    measure("events/note-on/p32", "operation", selection, |i| {
        f64::from(synth.note_on(black_box(note((i % 32) as i32)), 0.0, 1.0))
    });
    measure("parameters/unchanged", "operation", selection, |_| {
        f64::from(synth.set_parameters(black_box(values)))
    });
    measure(
        "parameters/resonance-changing",
        "operation",
        selection,
        |i| {
            values[11] = 0.05 + (i % 256) as f64 / 512.0;
            f64::from(synth.set_parameters(black_box(values)))
        },
    );
}

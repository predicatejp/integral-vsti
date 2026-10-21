// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// lookups.rsのモジュール定義・計測関数に連結する実験用コード。
#[path = "src/float_mode.rs"]
mod float_mode;
mod simd;
#[path = "src/oscillator_tables.rs"]
mod tables;

fn settings(poly: usize, unison: usize, waveform: usize, kind: usize) -> [f64; 12] {
    let mut p = parameters::defaults();
    p[0] = parameters::to_normalized(0, waveform as f64);
    p[6] = parameters::to_normalized(6, unison as f64);
    p[7] = 0.3;
    p[8] = parameters::to_normalized(8, poly as f64);
    p[9] = parameters::to_normalized(9, kind as f64);
    p
}

fn voice_note(id: usize) -> Note {
    Note {
        id: id as i32,
        channel: 0,
        pitch: 48 + (id % 32) as i16,
    }
}

fn run() {
    println!("case,iterations,min_ns,median_ns,max_ns");
    for (poly, unison, waveform, kind, mode) in [
        (1, 1, 0, 2, "steady"),
        (1, 7, 0, 2, "steady"),
        (4, 7, 0, 2, "steady"),
        (8, 1, 0, 2, "steady"),
        (32, 1, 0, 2, "steady"),
        (32, 7, 0, 2, "steady"),
        (31, 3, 0, 2, "steady"),
        (32, 7, 1, 2, "steady"),
        (32, 7, 2, 2, "steady"),
        (32, 7, 3, 2, "steady"),
        (32, 7, 0, 0, "steady"),
        (32, 7, 0, 1, "steady"),
        (32, 7, 0, 2, "detune"),
        (32, 7, 0, 2, "resonance"),
        (32, 7, 0, 2, "idle"),
        (32, 7, 0, 2, "sparse"),
        (32, 7, 0, 2, "released"),
    ] {
        let mut synth = Synthesizer::new();
        synth.set_sample_rate(48000.0);
        let mut p = settings(poly, unison, waveform, kind);
        synth.set_parameters(p);
        if mode != "idle" {
            for i in 0..if mode == "sparse" { 1 } else { poly } {
                synth.note_on(voice_note(i), 0.0, 1.0);
            }
        }
        for _ in 0..48000 {
            black_box(synth.generate_sample());
        }
        if mode == "released" {
            for i in 0..poly {
                synth.note_off(voice_note(i));
            }
            for _ in 0..96000 {
                black_box(synth.generate_sample());
            }
        }
        measure(
            &format!("synth/p{poly}/u{unison}/w{waveform}/f{kind}/{mode}"),
            |i| {
                if mode == "detune" {
                    p[7] = (5.0 + (i % 1000) as f64 * 0.075) / 100.0;
                }
                if mode == "resonance" {
                    p[11] = 0.05 + (i % 256) as f64 / 512.0;
                }
                if matches!(mode, "detune" | "resonance") {
                    synth.set_parameters(black_box(p));
                }
                black_box(&mut synth).generate_sample()
            },
        );
    }
    let mut lanes = simd::SvfLanes {
        x: [0.5; simd::LANES],
        state: simd::SvfLaneState {
            s1: [0.0; simd::LANES],
            s2: [0.0; simd::LANES],
        },
        g: std::array::from_fn(|i| {
            (std::f64::consts::PI * (440.0 + 100.0 * i as f64) / 48000.0).tan()
        }),
        damping_plus_g: [0.0; simd::LANES],
        denominator: [0.0; simd::LANES],
    };
    for i in 0..simd::LANES {
        lanes.damping_plus_g[i] = 1.0 / 0.707 + lanes.g[i];
        lanes.denominator[i] = 1.0 + lanes.g[i] * lanes.damping_plus_g[i];
    }
    measure("svf/contiguous/group", |i| {
        lanes.x.fill(if i % 2 == 0 { 0.5 } else { -0.5 });
        black_box(black_box(&mut lanes).process_sample(0)).0[0]
    });
    let input: [f64; 256] = std::array::from_fn(|i| (i as f64).sin() * 10.0);
    let (mut left, mut right) = ([0.0_f32; 256], [0.0_f32; 256]);
    measure("output/scalar/block256", |_| {
        for (i, x) in black_box(&input).iter().enumerate() {
            left[i] = *x as f32;
            right[i] = left[i];
        }
        black_box((&left, &right));
        0.0
    });
    measure("output/explicit/block256", |_| {
        simd::convert_samples(black_box(&input), &mut left, &mut right);
        black_box((&left, &right));
        0.0
    });
}

fn verify_kernels() {
    use oscillator::Waveform;
    // ユニゾン数1〜7、全波形、象限境界・端数レーン・位相ラップを検証。
    for count in 1..=7 {
        for kind in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for n in 0..2051 {
                let mut phases: [f64; 7] =
                    std::array::from_fn(|i| ((n + i * 257) % 2051) as f64 / 2051.0);
                if n < 4 {
                    phases[0] = n as f64 * 0.25;
                }
                let expected: f64 = phases[..count].iter().map(|&p| kind.unit_sample(p)).sum();
                // SAFETY: 上で生成した全位相は[0, 1)。
                assert_eq!(
                    unsafe { simd::generate_unison_sample(&phases[..count], kind) },
                    expected
                );
                let increments: [f64; 7] = std::array::from_fn(|i| (i + 1) as f64 / 8.0);
                let original = phases;
                simd::update_phase_sample(&mut phases[..count], &increments);
                for i in 0..7 {
                    assert_eq!(
                        phases[i],
                        if i < count {
                            (original[i] + increments[i]).fract()
                        } else {
                            original[i]
                        }
                    );
                }
            }
        }
    }
    let input: [f64; 259] = std::array::from_fn(|i| match i {
        0 => f64::NAN,
        1 => f64::INFINITY,
        2 => -0.0,
        _ => (i as f64).sin() * 1e5,
    });
    let (mut left, mut right) = ([0.0_f32; 259], [0.0_f32; 259]);
    simd::convert_samples(&input, &mut left, &mut right);
    for i in 0..259 {
        if input[i].is_nan() {
            assert!(left[i].is_nan() && right[i].is_nan());
        } else {
            assert_eq!(left[i].to_bits(), (input[i] as f32).to_bits());
            assert_eq!(left[i].to_bits(), right[i].to_bits());
        }
    }
    eprintln!("kernel verification passed; lanes={}", simd::LANES);
}

fn audio() {
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    // 波形・段数・端数Voice・ユニゾン数・レート・毎サンプル変調とノート境界を比較。
    for rate in [44100.0, 48000.0, 96000.0] {
        for waveform in 0..4 {
            for kind in 0..7 {
                for (poly, unison) in [(1, 1), (3, 3), (4, 4), (7, 7), (31, 7), (32, 7)] {
                    let mut s = Synthesizer::new();
                    s.set_sample_rate(rate);
                    let mut p = settings(poly, unison, waveform, kind);
                    p[1] = 0.0;
                    p[3] = 0.0;
                    p[4] = 1.0;
                    p[5] = parameters::to_normalized(5, 1.0);
                    s.set_parameters(p);
                    for i in 0..poly {
                        s.note_on(voice_note(i), i as f32 * 0.1, 0.4 + i as f32 * 0.01);
                    }
                    for n in 0..512 {
                        if n % 17 == 0 {
                            p[7] = (n % 101) as f64 / 100.0;
                            p[10] = parameters::to_normalized(
                                10,
                                (-240 + (n as i32 * 3) % 1201) as f64 * 10.0,
                            );
                            p[11] = 0.05 + (n % 97) as f64 / 120.0;
                            s.set_parameters(p);
                        }
                        if n == 64 {
                            for i in (0..poly).step_by(2) {
                                s.note_off(voice_note(i));
                            }
                        }
                        if n == 96 {
                            s.note_on(voice_note(63), -23.7, 0.8);
                        }
                        if n == 128 {
                            p[8] = 0.0;
                            s.set_parameters(p);
                        }
                        if n == 160 {
                            p[8] = parameters::to_normalized(8, poly as f64);
                            s.set_parameters(p);
                            s.note_on(voice_note(1), 0.0, 1.0);
                        }
                        if n == 224 {
                            s.stop();
                        }
                        if n == 256 {
                            s.note_on(voice_note(4), 0.0, 1.0);
                        }
                        let v = s.generate_sample();
                        assert!(v.is_finite());
                        out.write_all(&v.to_le_bytes()).unwrap();
                    }
                }
            }
        }
    }
}

fn main() {
    // 全構成で同じ環境。CPU検出はrunnerが汎用ISAのprobeを先に実行する。
    let _mode = float_mode::FloatMode::flush_subnormals();
    match std::env::args().nth(1).as_deref() {
        Some("verify") => verify_kernels(),
        Some("audio") => audio(),
        _ => run(),
    }
}

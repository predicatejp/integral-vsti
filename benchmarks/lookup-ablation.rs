// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// run-lookup-ablation.pyがlookups.rsの計測関数・DSPモジュール定義に連結する。

fn interpolate(position: f64, table: &[f64]) -> f64 {
    let i = (position as usize).min(table.len() - 2);
    table[i] + (table[i + 1] - table[i]) * (position - i as f64)
}

fn detune_ratio(cents: f64) -> f64 {
    interpolate(cents + 100.0, &lookup_data::DETUNE)
}

fn prewarp(normalized_frequency: f64) -> f64 {
    interpolate(
        normalized_frequency * (4096.0 / 0.49),
        &lookup_data::PREWARP,
    )
}

fn cutoff_ratio(cents: f64) -> f64 {
    lookup_data::CUTOFF[((cents + 2400.0) / 10.0) as usize]
}

fn emit_tables() {
    for (name, values) in [
        (
            "NOTES",
            (0..128).map(|n| note_formula(n, 0.0)).collect::<Vec<_>>(),
        ),
        (
            "SINE",
            (0..=1024)
                .map(|n| (std::f64::consts::FRAC_PI_2 * n as f64 / 1024.0).sin())
                .collect(),
        ),
        ("GAIN", (1..=7).map(|n| 1.0 / (n as f64).sqrt()).collect()),
        (
            "CUTOFF",
            (0..=1200)
                .map(|n| 2.0_f64.powf((n as f64 * 10.0 - 2400.0) / 1200.0))
                .collect(),
        ),
        (
            "DETUNE",
            (0..=200)
                .map(|n| 2.0_f64.powf((n as f64 - 100.0) / 1200.0))
                .collect(),
        ),
        (
            "PREWARP",
            (0..=4096)
                .map(|n| (std::f64::consts::PI * (n as f64 * 0.49 / 4096.0)).tan())
                .collect(),
        ),
        (
            "TIME",
            (0..=371)
                .map(|n| {
                    if n == 0 {
                        0.0
                    } else {
                        10.0_f64.powf((n as f64 - 1.0) / 100.0).min(5000.0)
                    }
                })
                .collect(),
        ),
    ] {
        println!("pub const {name}: [f64; {}] = {values:?};", values.len());
    }
}

fn automation_values(i: usize) -> [f64; 12] {
    let mut values = parameters::defaults();
    values[6] = 1.0;
    values[8] = 1.0;
    values[9] = parameters::to_normalized(9, 2.0);
    values[7] = (5.0 + (i % 1000) as f64 * 0.075) / 100.0;
    values[10] = parameters::to_normalized(10, (i % 241) as f64 * 10.0);
    values[11] = 0.05 + (i % 256) as f64 / 512.0;
    values
}

fn ablation(rate: f64) {
    // 既存の定常発音3条件・Note On2条件を同じ計測関数で比較する。
    integrated(rate);
    let mut synth = make_synth(rate, 32, 7);
    measure("synth/p32/u7/automation", |i| {
        synth.set_parameters(black_box(automation_values(i)));
        black_box(&mut synth).generate_sample()
    });
    measure("parameters/changing", |i| {
        f64::from(black_box(&mut synth).set_parameters(black_box(automation_values(i))))
    });
    measure("ui/time", |i| {
        parameters::slider_to_plain(black_box(1), black_box((i % 372) as u32))
    });
}

fn verify() {
    for n in 0..128 {
        assert_eq!(lookup_data::NOTES[n], note_formula(n, 0.0));
    }
    for n in 0..=1200 {
        let cents = n as f64 * 10.0 - 2400.0;
        assert_eq!(cutoff_ratio(cents), 2.0_f64.powf(cents / 1200.0));
    }
    for n in 0..=371 {
        let exact = if n == 0 {
            0.0
        } else {
            10.0_f64.powf((n as f64 - 1.0) / 100.0).min(5000.0)
        };
        assert_eq!(lookup_data::TIME[n], exact);
    }
    let (mut detune_error, mut prewarp_error, mut sine_error) = (0.0_f64, 0.0_f64, 0.0_f64);
    for i in 0..=200_003 {
        let fraction = i as f64 / 200_003.0;
        let cents = -100.0 + 200.0 * fraction;
        let exact = 2.0_f64.powf(cents / 1200.0);
        detune_error = detune_error.max((detune_ratio(cents) / exact - 1.0).abs());
        let f = fraction * 0.49;
        if f > 0.0 {
            let exact = (std::f64::consts::PI * f).tan();
            prewarp_error = prewarp_error.max((prewarp(f) / exact - 1.0).abs());
        }
        if fraction < 1.0 {
            sine_error = sine_error.max(
                (quarter_sine(fraction, &lookup_data::SINE)
                    - (std::f64::consts::TAU * fraction).sin())
                .abs(),
            );
        }
    }
    assert!(detune_error < 4.18e-8 && prewarp_error < 3.54e-5 && sine_error < 2.95e-7);
    println!(
        "detune_max_relative={detune_error:.12e}\nprewarp_max_relative={prewarp_error:.12e}\nsine_max_absolute={sine_error:.12e}"
    );
    // テーブル使用時も各応答・Qの両端・サンプルレートで有限な応答を得る。
    for rate in [44100.0, 48000.0, 96000.0] {
        for kind in parameters::FILTER_TYPES {
            for q in [0.5, 10.0] {
                let mut filter = filter::Filter::new();
                for n in 0..4096 {
                    let output = filter.process_sample(
                        if n == 0 { 1.0 } else { 0.0 },
                        kind,
                        &filter::FilterParameters {
                            cutoff_hz: 20.0 + n as f64 * 5.0,
                            resonance_q: q,
                        },
                        rate,
                    );
                    assert!(output.is_finite());
                }
            }
        }
    }
}

fn audio() {
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
        let mut synth = make_synth(rate, 32, 7);
        for n in 0..8192 {
            synth.set_parameters(automation_values(n));
            let sample = synth.generate_sample();
            assert!(sample.is_finite());
            out.write_all(&sample.to_le_bytes()).unwrap();
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args[1].as_str() {
        "tables" => emit_tables(),
        "verify" => verify(),
        "audio" => audio(),
        rate => ablation(rate.parse().unwrap()),
    }
}

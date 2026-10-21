#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""7候補の追加・除外比較。生成・計測はtarget内のコピーだけで行う。"""
import argparse
import csv
import json
import math
from pathlib import Path
import random
import shutil
import struct
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target/lookup-ablation"
CANDIDATES = ["notes", "sine", "gain", "cutoff", "detune", "prewarp", "time"]
parser = argparse.ArgumentParser()
parser.add_argument("--cpu", type=int)
args = parser.parse_args()
prefix = [] if args.cpu is None else ["taskset", "-c", str(args.cpu)]
OUT.mkdir(parents=True, exist_ok=True)
source_prefix = (ROOT / "benchmarks/lookups.rs").read_text().split("fn main() {")[0]
source = source_prefix.replace('../src/', 'src/') + '\nmod lookup_data;\n' + (ROOT / "benchmarks/lookup-ablation.rs").read_text()


def compile_rust(directory):
    subprocess.run(["rustc", "--edition=2024", "-D", "warnings", "-C", "opt-level=3",
                    "-C", "panic=abort", "-C", "codegen-units=16", str(directory / "experiment.rs"),
                    "-o", str(directory / "bench")], check=True)


def replace(directory, filename, old, new):
    path = directory / "src" / filename
    text = path.read_text()
    assert text.count(old) == 1, f"source changed: {filename}: {old}"
    path.write_text(text.replace(old, new))


# Rustで生成した定数を全構成に共通で使用する。生成器の型検査用配列は計測しない。
generator = OUT / "generator"
shutil.copytree(ROOT / "src", generator / "src", dirs_exist_ok=True)
(generator / "experiment.rs").write_text(source)
(generator / "lookup_data.rs").write_text('\n'.join(
    f'pub const {name}: [f64; {size}] = [0.0; {size}];'
    for name, size in [("NOTES", 128), ("SINE", 1025), ("GAIN", 7), ("CUTOFF", 1201),
                       ("DETUNE", 201), ("PREWARP", 4097), ("TIME", 372)]))
compile_rust(generator)
tables = subprocess.check_output([str(generator / "bench"), "tables"])
variants = {"none": [], "all": CANDIDATES}
variants.update({f"only-{c}": [c] for c in CANDIDATES})
variants.update({f"without-{c}": [v for v in CANDIDATES if v != c] for c in CANDIDATES})
(OUT / "variants.json").write_text(json.dumps(variants, indent=2) + '\n')
for name, selected in variants.items():
    print(f"build {name}", flush=True)
    directory = OUT / name
    shutil.copytree(ROOT / "src", directory / "src", dirs_exist_ok=True)
    (directory / "experiment.rs").write_text(source)
    (directory / "lookup_data.rs").write_bytes(tables)
    if "notes" in selected:
        replace(directory, "voice.rs",
                "440.0 * 2.0_f64.powf((f64::from(note.pitch) - 69.0 + f64::from(tuning) / 100.0) / 12.0)",
                "crate::note_lookup(note.pitch as usize, f64::from(tuning), &crate::lookup_data::NOTES)")
    if "sine" in selected:
        replace(directory, "oscillator.rs", "(std::f64::consts::TAU * phase).sin()",
                "crate::quarter_sine(phase, &crate::lookup_data::SINE)")
    if "gain" in selected:
        replace(directory, "oscillator.rs", "sum / (count as f64).sqrt()",
                "sum * crate::lookup_data::GAIN[count - 1]")
    if "cutoff" in selected:
        replace(directory, "voice.rs", "2.0_f64.powf(cutoff_cents / 1200.0)",
                "crate::cutoff_ratio(cutoff_cents)")
        replace(directory, "voice.rs", "2.0_f64.powf(parameters::to_plain(10, values[10]) / 1200.0)",
                "crate::cutoff_ratio(parameters::to_plain(10, values[10]))")
    if "detune" in selected:
        replace(directory, "oscillator.rs", "        2.0_f64.powf(cents / 1200.0)\n    }", "        crate::detune_ratio(cents)\n    }")
    if "prewarp" in selected:
        replace(directory, "filter.rs", "(std::f64::consts::PI * (cutoff_hz / sample_rate).min(0.49)).tan()",
                "crate::prewarp((cutoff_hz / sample_rate).min(0.49))")
    if "time" in selected:
        replace(directory, "parameters.rs", '''if tick == 0 {
            0.0
        } else {
            10.0_f64.powf((f64::from(tick) - 1.0) / 100.0).min(5000.0)
        }''', 'crate::lookup_data::TIME[tick.min(371) as usize]')
    compile_rust(directory)
    with (directory / "verification.txt").open('w') as output:
        subprocess.run([*prefix, str(directory / "bench"), "verify"], stdout=output, check=True)

# 各ラウンドの順序を変えて固定順による偏りを抑える。CPU計測は常に1プロセスずつ。
orders = []
for repeat in range(3):
    order = list(variants)
    random.Random(20261021 + repeat).shuffle(order)
    orders.append(order)
    for name in order:
        for rate in (48000, 96000):
            print(f"measure {repeat} {name} {rate}", flush=True)
            with (OUT / f"{name}-{rate}-{repeat}.csv").open('w') as output:
                subprocess.run([*prefix, str(OUT / name / "bench"), str(rate)], stdout=output, check=True)
(OUT / "orders.json").write_text(json.dumps(orders, indent=2) + '\n')

reference = None
with (OUT / "audio-errors.csv").open('w') as output:
    writer = csv.writer(output)
    writer.writerow(["variant", "rate", "case", "max_absolute", "rms_error", "relative_rms_error"])
    for name in variants:
        data = subprocess.check_output([*prefix, str(OUT / name / "bench"), "audio"])
        samples = struct.unpack(f'<{len(data) // 8}d', data)
        assert len(samples) == 8 * 8192
        if reference is None:
            reference = samples
        for block in range(8):
            start, end = block * 8192, (block + 1) * 8192
            expected, actual = reference[start:end], samples[start:end]
            diff = [a - b for a, b in zip(actual, expected)]
            rms = math.sqrt(sum(d * d for d in diff) / 8192)
            signal = math.sqrt(sum(v * v for v in expected) / 8192)
            writer.writerow([name, 48000 if block < 4 else 96000,
                             ["p1/u1", "p1/u7", "p32/u7", "automation"][block % 4],
                             max(map(abs, diff)), rms, rms / signal])
print(OUT)

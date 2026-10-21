#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""本体のコピーだけを差し替えて計測する。生成物はtarget/lookup-experiment以下。"""
import argparse
import math
from pathlib import Path
import shutil
import struct
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target/lookup-experiment"
parser = argparse.ArgumentParser()
parser.add_argument("--cpu", type=int, help="Linuxのtasksetで固定するCPU")
args = parser.parse_args()
prefix = [] if args.cpu is None else ["taskset", "-c", str(args.cpu)]
OUT.mkdir(parents=True, exist_ok=True)


def compile_rust(source, binary):
    subprocess.run(["rustc", "--edition=2024", "-D", "warnings", "-C", "opt-level=3",
                    "-C", "panic=abort", "-C", "codegen-units=16", str(source), "-o", str(binary)], check=True)


def replace_once(path, old, new):
    text = path.read_text()
    assert text.count(old) == 1, f"source changed: {path.name}"
    path.write_text(text.replace(old, new))


binary = OUT / "micro"
compile_rust(ROOT / "benchmarks/lookups.rs", binary)
for repeat in range(2):
    with (OUT / f"micro-{repeat}.csv").open("w") as csv, (OUT / f"micro-{repeat}.meta").open("w") as meta:
        subprocess.run([*prefix, str(binary), "micro"], stdout=csv, stderr=meta, check=True)

reference = None
with (OUT / "audio-errors.csv").open("w") as errors:
    errors.write("variant,rate,polyphony,unison,max_abs,rms_error,relative_rms_error\n")
    for name, intervals, use_notes in [
        ("original", 1024, False), ("notes", 1024, True),
        ("sine64", 64, False), ("sine256", 256, False),
        ("sine1024", 1024, False), ("sine4096", 4096, False),
        ("both1024", 1024, True),
    ]:
        print(name, flush=True)
        directory = OUT / name
        shutil.copytree(ROOT / "src", directory / "src", dirs_exist_ok=True)
        source = directory / "experiment.rs"
        source.write_text((ROOT / "benchmarks/lookups.rs").read_text()
                          .replace('../src/', 'src/') + '\nmod lookup_data;\n')
        with (directory / "lookup_data.rs").open("w") as data:
            subprocess.run([str(binary), "tables", str(intervals)], stdout=data, check=True)
        if name.startswith("sine") or name.startswith("both"):
            replace_once(directory / "src/oscillator.rs", "(std::f64::consts::TAU * phase).sin()",
                         "crate::quarter_sine(phase, &crate::lookup_data::SINE)")
        if use_notes:
            replace_once(directory / "src/voice.rs",
                         "440.0 * 2.0_f64.powf((f64::from(note.pitch) - 69.0 + f64::from(tuning) / 100.0) / 12.0)",
                         "crate::note_lookup(note.pitch as usize, f64::from(tuning), &crate::lookup_data::NOTES)")
        program = directory / "bench"
        compile_rust(source, program)
        for repeat in range(2):
            for rate in (48000, 96000):
                with (OUT / f"{name}-{rate}-{repeat}.csv").open("w") as csv:
                    subprocess.run([*prefix, str(program), "synth", str(rate)], stdout=csv, check=True)
        audio = subprocess.check_output([*prefix, str(program), "audio"])
        (OUT / f"{name}.f64le").write_bytes(audio)
        samples = struct.unpack(f"<{len(audio) // 8}d", audio)
        if reference is None:
            reference = samples
        assert len(samples) == len(reference) == 6 * 8192
        for case in range(6):
            begin, end = case * 8192, (case + 1) * 8192
            actual, expected = samples[begin:end], reference[begin:end]
            differences = [a - b for a, b in zip(actual, expected)]
            rms = math.sqrt(sum(d * d for d in differences) / 8192)
            signal = math.sqrt(sum(v * v for v in expected) / 8192)
            polyphony, unison = [(1, 1), (1, 7), (32, 7)][case % 3]
            errors.write(f"{name},{48000 if case < 3 else 96000},{polyphony},{unison},"
                         f"{max(map(abs, differences)):.12e},{rms:.12e},{rms / signal:.12e}\n")
print(OUT)

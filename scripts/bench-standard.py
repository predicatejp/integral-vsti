#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Build, run and compare the fixed DSP performance regression suite."""

import argparse
import csv
import hashlib
import io
import json
import math
import os
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
FLAGS = ["--edition=2024", "-D", "warnings", "-C", "opt-level=3",
         "-C", "panic=abort", "-C", "codegen-units=16"]


def capture(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/performance/current.json")
    parser.add_argument("--baseline", type=Path, help="Compare against a saved run with identical conditions")
    parser.add_argument("--max-regression-percent", type=float, default=10.0)
    parser.add_argument("--rate", type=int, choices=[44100, 48000, 96000], default=48000)
    parser.add_argument("--block", type=int, default=256)
    parser.add_argument("--rounds", type=int, choices=range(3, 32, 2), default=5)
    parser.add_argument("--seconds", type=int, choices=range(4, 61), default=4)
    parser.add_argument("--case", default="", help="Case-name prefix")
    parser.add_argument("--cpu", type=int, help="Pin to this logical CPU (Linux)")
    parser.add_argument("--list", action="store_true", help="List case names")
    args = parser.parse_args()
    if not 1 <= args.block <= 2048:
        parser.error("--block must be 1..2048")
    if not math.isfinite(args.max_regression_percent) or args.max_regression_percent < 0:
        parser.error("--max-regression-percent must be finite and nonnegative")
    if args.baseline and args.output.resolve() == args.baseline.resolve():
        parser.error("--output must differ from --baseline")
    if args.cpu is not None:
        if not hasattr(os, "sched_setaffinity"):
            parser.error("--cpu requires Linux; otherwise set affinity outside this script")
        if args.cpu not in os.sched_getaffinity(0):
            parser.error("--cpu is outside this process's allowed CPU set")
        os.sched_setaffinity(0, {args.cpu})

    cpu = platform.processor()
    if Path("/proc/cpuinfo").is_file():
        cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                    if line.startswith("model name")), cpu)
    conditions = {
        "schema": 1,
        "suite_sha256": hashlib.sha256((ROOT / "benchmarks/standard.rs").read_bytes()).hexdigest(),
        "rustc": capture("rustc", "--version", "--verbose"),
        "flags": FLAGS,
        "platform": platform.platform(), "cpu": cpu,
        "affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "rate": args.rate, "block": args.block, "rounds": args.rounds,
        "seconds": args.seconds, "warmup_seconds": 2, "case_prefix": args.case,
    }
    baseline = None
    if args.baseline:
        baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
        differences = [k for k, v in conditions.items() if baseline["conditions"].get(k) != v]
        if differences:
            parser.error("Baseline conditions differ: " + ", ".join(differences))

    binary = ROOT / "target" / ("bench-standard.exe" if os.name == "nt" else "bench-standard")
    binary.parent.mkdir(exist_ok=True)
    subprocess.run(["rustc", *FLAGS, "benchmarks/standard.rs", "-o", str(binary)], cwd=ROOT, check=True)
    names = capture(str(binary), "--list").splitlines()
    if args.list:
        print("\n".join(names))
        return 0
    if not any(name.startswith(args.case) for name in names):
        parser.error("No matching cases")
    raw = capture(str(binary), str(args.rate), str(args.block), str(args.rounds), str(args.seconds), args.case)
    results = {}
    for row in csv.DictReader(io.StringIO(raw)):
        results[row["case"]] = {k: row[k] if k == "fingerprint" else float(row[k])
                                for k in row if k != "case"}
    expected = {name for name in names if name.startswith(args.case)}
    if set(results) != expected:
        raise RuntimeError("Incomplete benchmark results")
    result = {"conditions": conditions, "commit": capture("git", "rev-parse", "HEAD"),
              "dirty": bool(capture("git", "status", "--porcelain")), "cases": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{'case':25} {'median ns':>10} {'min..max ns':>21} {'change':>9}  audio")
    failed = False
    if baseline and set(baseline["cases"]) != set(results):
        parser.error("Baseline cases differ")
    for name, row in results.items():
        change, audio = "", ""
        if baseline:
            old = baseline["cases"][name]
            delta = (row["median_ns"] / old["median_ns"] - 1.0) * 100.0
            change = f"{delta:+.1f}%"
            if delta > args.max_regression_percent:
                failed = True
                change += " !"
            audio = "same" if row["fingerprint"] == old["fingerprint"] else "changed"
        spread = f"{row['min_ns']:.1f}..{row['max_ns']:.1f}"
        print(f"{name:25} {row['median_ns']:10.1f} {spread:>21} {change:>9}  {audio}")
    print(f"Result: {args.output}")
    if failed:
        print(f"Regression exceeds {args.max_regression_percent:g}%", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

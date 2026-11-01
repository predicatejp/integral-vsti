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
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
FLAGS = ["--edition=2024", "-D", "warnings", "-C", "opt-level=3",
         "-C", "panic=abort", "-C", "codegen-units=16"]


def capture(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def collect_results(raw, names, rounds, burn_in, interval):
    results = {name: {"samples_ns": []} for name in names}
    trials = []
    rows = list(csv.DictReader(io.StringIO(raw)))
    if len(rows) != rounds * len(names):
        raise ValueError("Incomplete benchmark results")
    for trial in range(rounds):
        batch = rows[trial * len(names):(trial + 1) * len(names)]
        started, finished = int(batch[0]["started_ns"]), int(batch[0]["finished_ns"])
        earliest = burn_in * 10**9 if trial == 0 else trials[-1]["finished_ns"] + interval * 10**9
        if started < earliest or finished <= started:
            raise ValueError("Insufficient burn-in or trial interval")
        trials.append({"number": trial + 1, "started_ns": started, "finished_ns": finished})
        if [row["case"] for row in batch] != names:
            raise ValueError("Unexpected benchmark cases")
        for row in batch:
            if (int(row["trial"]), int(row["started_ns"]), int(row["finished_ns"])) != (trial + 1, started, finished):
                raise ValueError("Inconsistent trial metadata")
            ns, peak, rms = (float(row[k]) for k in ["ns", "peak", "rms"])
            if not all(math.isfinite(v) for v in [ns, peak, rms]) or ns <= 0 or not 0 <= rms <= peak:
                raise ValueError("Invalid benchmark measurement")
            result = results[row["case"]]
            audio = {"fingerprint": row["fingerprint"], "peak": peak, "rms": rms}
            if trial and any(result[k] != v for k, v in audio.items()):
                raise ValueError("Non-reproducible audio")
            result.update(audio)
            result["samples_ns"].append(ns)
    for result in results.values():
        samples = result["samples_ns"]
        result.update(min_ns=min(samples), median_ns=statistics.median(samples), max_ns=max(samples))
    return results, trials


def report_results(results, baseline):
    if baseline is not None and set(baseline) != set(results):
        raise ValueError("Baseline cases differ")
    print(f"{'case':25} {'median ns':>10} {'min..max ns':>21} {'change':>10}  audio")
    worse = []
    for name, row in results.items():
        change, audio = "", ""
        if baseline is not None:
            old = baseline[name]
            delta = (row["median_ns"] / old["median_ns"] - 1.0) * 100.0
            change = f"{delta:+.3f}%"
            if row["median_ns"] > old["median_ns"]:
                worse.append(name)
                change += " !"
            audio = "same" if row["fingerprint"] == old["fingerprint"] else "changed"
        spread = f"{row['min_ns']:.1f}..{row['max_ns']:.1f}"
        print(f"{name:25} {row['median_ns']:10.1f} {spread:>21} {change:>10}  {audio}")
    if worse:
        print("Review required: median increased in " + ", ".join(worse), file=sys.stderr)
    return 1 if worse else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/performance/current.json")
    parser.add_argument("--baseline", type=Path, help="Compare against a saved run with identical conditions")
    parser.add_argument("--rate", type=int, choices=[44100, 48000, 96000], default=48000)
    parser.add_argument("--block", type=int, default=256)
    parser.add_argument("--rounds", type=int, default=10)
    parser.add_argument("--burn-in-seconds", type=int, default=30, help="Wall-clock DSP burn-in, at least 30s")
    parser.add_argument("--interval-seconds", type=int, default=5, help="Sleep between complete trials, at least 1s")
    parser.add_argument("--seconds", type=int, choices=range(4, 61), default=4)
    parser.add_argument("--case", default="", help="Case-name prefix")
    parser.add_argument("--cpu", type=int, help="Pin to this logical CPU (Linux)")
    parser.add_argument("--list", action="store_true", help="List case names")
    args = parser.parse_args()
    if not 10 <= args.rounds <= 100:
        parser.error("--rounds must be 10..100")
    if args.burn_in_seconds < 30 or args.interval_seconds < 1:
        parser.error("--burn-in-seconds must be >=30 and --interval-seconds must be >=1")
    if not 1 <= args.block <= 2048:
        parser.error("--block must be 1..2048")
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
        "schema": 2,
        "suite_sha256": hashlib.sha256((ROOT / "benchmarks/standard.rs").read_bytes()).hexdigest(),
        "rustc": capture("rustc", "--version", "--verbose"),
        "flags": FLAGS,
        "platform": platform.platform(), "cpu": cpu,
        "affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "rate": args.rate, "block": args.block, "rounds": args.rounds,
        "seconds": args.seconds, "warmup_seconds": 2, "case_prefix": args.case,
        "burn_in_seconds": args.burn_in_seconds, "interval_seconds": args.interval_seconds,
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
    raw = capture(str(binary), str(args.rate), str(args.block), str(args.rounds), str(args.seconds), args.case,
                  str(args.burn_in_seconds), str(args.interval_seconds))
    results, trials = collect_results(raw, [name for name in names if name.startswith(args.case)],
                                     args.rounds, args.burn_in_seconds, args.interval_seconds)
    result = {"conditions": conditions, "commit": capture("git", "rev-parse", "HEAD"),
              "dirty": bool(capture("git", "status", "--porcelain")), "trials": trials, "cases": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Result: {args.output}")
    return report_results(results, baseline["cases"] if baseline else None)


if __name__ == "__main__":
    sys.exit(main())

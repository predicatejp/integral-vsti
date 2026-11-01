#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""Randomized blocked before/after DSP measurements with paired t-tests."""

import argparse
import csv
import hashlib
import io
import json
import math
import os
from pathlib import Path
import platform
import random
import secrets
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import numpy as np
import scipy
from scipy import stats

ROOT = Path(__file__).resolve().parent.parent
FLAGS = ["--edition=2024", "-D", "warnings", "-C", "opt-level=3",
         "-C", "panic=abort", "-C", "codegen-units=16"]
SIDES = ("before", "after")
ALPHA = 0.05


def capture(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def build(tree, destination, harness):
    # 同一ハーネスと、計測開始前に固定した各実装のソースを使う。
    shutil.copytree(tree / "src", destination / "src")
    (destination / "benchmarks").mkdir()
    (destination / "benchmarks/standard.rs").write_bytes(harness)
    digest = hashlib.sha256()
    for path in sorted((destination / "src").rglob("*")):
        if path.is_file():
            digest.update(path.relative_to(destination).as_posix().encode() + b"\0")
            digest.update(path.read_bytes())
    metadata = {"commit": capture("git", "rev-parse", "HEAD", cwd=tree),
                "dirty": bool(capture("git", "status", "--porcelain", "--", "src", cwd=tree)),
                "source_sha256": digest.hexdigest()}
    binary = destination / ("standard.exe" if os.name == "nt" else "standard")
    subprocess.run(["rustc", *FLAGS, "benchmarks/standard.rs", "-o", str(binary)],
                   cwd=destination, check=True)
    metadata["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    return binary, metadata


def measure(binary, case, rate, buffer, audio_seconds, burn_in_ms):
    raw = capture(str(binary), str(rate), str(buffer), str(audio_seconds), case, str(burn_in_ms))
    rows = list(csv.DictReader(io.StringIO(raw)))
    if len(rows) != 1 or rows[0]["case"] != case:
        raise ValueError("Unexpected benchmark case")
    row = rows[0]
    result = {k: float(row[k]) for k in ["ns", "peak", "rms"]}
    result.update({k: int(row[k]) for k in ["burn_in_ns", "burn_in_cycles", "measurement_ns"]})
    result["fingerprint"] = row["fingerprint"]
    if (not all(math.isfinite(result[k]) for k in ["ns", "peak", "rms"])
            or result["ns"] <= 0 or not 0 <= result["rms"] <= result["peak"]
            or result["burn_in_ns"] < burn_in_ms * 10**6
            or result["burn_in_cycles"] < 1 or result["measurement_ns"] <= 0):
        raise ValueError("Invalid measurement or insufficient burn-in")
    return result


def run_blocks(names, measure_case, count, cooldown, seed, *, clock=time.perf_counter_ns, sleep=time.sleep):
    rng = random.Random(seed)
    origin = clock()
    blocks = []
    audio = {}
    for number in range(count):
        if blocks:
            sleep(cooldown)
        started = clock() - origin
        if blocks and started - blocks[-1]["finished_ns"] < cooldown * 10**9:
            raise ValueError("Insufficient block cooldown")
        jobs = [(name, side) for name in names for side in SIDES]
        rng.shuffle(jobs)
        print(f"Block {number + 1}/{count}: {len(jobs)} randomized runs", file=sys.stderr)
        runs = []
        for name, side in jobs:
            run_start = clock() - origin
            result = measure_case(side, name)
            run_end = clock() - origin
            check = tuple(result[k] for k in ["fingerprint", "peak", "rms"])
            key = (name, side)
            if key in audio and audio[key] != check:
                raise ValueError("Non-reproducible audio: " + name + "/" + side)
            audio[key] = check
            runs.append(dict(case=name, side=side, started_ns=run_start, finished_ns=run_end, **result))
        blocks.append(dict(number=number + 1, started_ns=started, finished_ns=clock() - origin, runs=runs))
    return blocks


def summary(values):
    return dict(mean=statistics.mean(values), sd=statistics.stdev(values),
                median=statistics.median(values), min=min(values), max=max(values))


def paired_statistics(before, after):
    if len(before) != len(after) or len(before) < 10:
        raise ValueError("At least 10 complete block pairs are required")
    if not all(math.isfinite(x) and x > 0 for x in [*before, *after]):
        raise ValueError("Invalid sample time")
    differences = [a - b for b, a in zip(before, after)]
    delta = summary(differences)
    result = dict(blocks=len(before), before=summary(before), after=summary(after),
                  difference=delta, differences_ns=differences, df=len(before) - 1,
                  t=None, p=None, ci95=None, shapiro_p=None, lag1=None)
    # 差の分散が0のときはt検定を定義できない。p=1で同等と扱わない。
    if delta["sd"] == 0:
        return result
    test = stats.ttest_rel(after, before, alternative="two-sided", nan_policy="raise")
    ci = test.confidence_interval(0.95)
    result.update(t=float(test.statistic), p=float(test.pvalue), ci95=[float(ci.low), float(ci.high)],
                  shapiro_p=float(stats.shapiro(differences).pvalue))
    if statistics.pstdev(differences[:-1]) and statistics.pstdev(differences[1:]):
        result["lag1"] = statistics.correlation(differences[:-1], differences[1:])
    return result


def holm(pvalues):
    # 全ケースを1つの比較族とする。未定義の検定もケース数から除外しない。
    ordered = sorted(range(len(pvalues)), key=lambda i: 1.0 if pvalues[i] is None else pvalues[i])
    adjusted = [None] * len(pvalues)
    previous = 0.0
    for rank, index in enumerate(ordered):
        if pvalues[index] is not None:
            previous = min(1.0, max(previous, (len(pvalues) - rank) * pvalues[index]))
            adjusted[index] = previous
    return adjusted


def analyze(blocks, names):
    samples = {name: {side: [] for side in SIDES} for name in names}
    expected = {(name, side) for name in names for side in SIDES}
    audio = {}
    for number, block in enumerate(blocks, 1):
        runs = block["runs"]
        if (block["number"] != number or len(runs) != len(expected)
                or {(r["case"], r["side"]) for r in runs} != expected):
            raise ValueError("Missing or duplicate block pairs")
        for run in runs:
            key = (run["case"], run["side"])
            check = tuple(run[k] for k in ["fingerprint", "peak", "rms"])
            if key in audio and audio[key] != check:
                raise ValueError("Non-reproducible audio")
            audio[key] = check
            samples[run["case"]][run["side"]].append(run["ns"])
    results = {name: paired_statistics(**values) for name, values in samples.items()}
    for (name, result), p in zip(results.items(), holm([r["p"] for r in results.values()])):
        result["p_holm"] = p
        result["audio_same"] = audio[(name, "before")] == audio[(name, "after")]
        result["status"] = ("undefined" if p is None else
                            "not_detected" if p >= ALPHA else
                            "slower" if result["difference"]["mean"] > 0 else "faster")
    return results


def report(results):
    for name, r in results.items():
        print(f"{name}: blocks={r['blocks']}, df={r['df']}, {r['status']}, audio={'same' if r['audio_same'] else 'changed'}")
        for key in ["before", "after", "difference"]:
            x = r[key]
            print(f"  {key:10} mean={x['mean']:.4f} sd={x['sd']:.4f} median={x['median']:.4f} min={x['min']:.4f} max={x['max']:.4f} ns")
        print(f"  t={r['t']} p(two-sided)={r['p']} p(Holm)={r['p_holm']} CI95(per case)={r['ci95']}")
        print(f"  diagnostics: Shapiro p={r['shapiro_p']}, difference lag-1 r={r['lag1']}")
    print("not_detected means no detected difference, not equivalence or approval.")
    return int(any(r["status"] in ["slower", "undefined"] for r in results.values()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, help="Git worktree for the implementation before the change")
    parser.add_argument("--after", type=Path, default=ROOT, help="Git worktree after the change")
    parser.add_argument("--output", type=Path, default=ROOT / "target/performance/paired.json")
    parser.add_argument("--blocks", type=int, default=10)
    parser.add_argument("--burn-in-ms", type=int, default=10,
                        help="Wall-clock minimum, including at least one full scenario pass")
    parser.add_argument("--cooldown-seconds", type=int, default=5)
    parser.add_argument("--rate", type=int, choices=[44100, 48000, 96000], default=48000)
    parser.add_argument("--block", type=int, default=256, help="Host audio buffer size, not experiment blocks")
    parser.add_argument("--audio-seconds", type=int, choices=range(4, 61), default=4,
                        help="Generated audio duration, not wall-clock measurement time")
    parser.add_argument("--case", action="append", default=[], help="Case-name prefix (repeatable)")
    parser.add_argument("--seed", type=int, help="Replay an execution order; default is a fresh random seed")
    parser.add_argument("--cpu", type=int, help="Pin both implementations to this logical CPU (Linux)")
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()
    if not args.list and args.before is None:
        parser.error("--before is required")
    if args.blocks < 10 or args.burn_in_ms < 10 or args.cooldown_seconds < 3:
        parser.error("Require >=10 blocks, >=10ms burn-in per run and >=3s cooldown")
    if not 1 <= args.block <= 2048:
        parser.error("--block must be 1..2048")
    if args.cpu is not None:
        if not hasattr(os, "sched_setaffinity") or args.cpu not in os.sched_getaffinity(0):
            parser.error("--cpu requires an allowed Linux logical CPU")
        os.sched_setaffinity(0, {args.cpu})
    seed = args.seed if args.seed is not None else secrets.randbits(64)
    cpu = platform.processor()
    if Path("/proc/cpuinfo").is_file():
        cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                    if line.startswith("model name")), cpu)
    harness = (ROOT / "benchmarks/standard.rs").read_bytes()
    (ROOT / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="paired-bench-", dir=ROOT / "target") as temp:
        binaries, implementations = {}, {}
        for side in (["after"] if args.list else SIDES):
            tree = getattr(args, side).resolve()
            binaries[side], implementations[side] = build(tree, Path(temp) / side, harness)
        names = capture(str(binaries["after"]), "--list").splitlines()
        if args.list:
            print("\n".join(names))
            return 0
        if capture(str(binaries["before"]), "--list").splitlines() != names:
            raise ValueError("Different case lists")
        names = [name for name in names if not args.case or any(name.startswith(p) for p in args.case)]
        if not names:
            parser.error("No matching cases")
        conditions = dict(schema=4, suite_sha256=hashlib.sha256(harness).hexdigest(),
                          rustc=capture("rustc", "--version", "--verbose"), flags=FLAGS,
                          platform=platform.platform(), cpu=cpu,
                          affinity=sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
                          python=platform.python_version(), numpy=np.__version__, scipy=scipy.__version__,
                          rate=args.rate, buffer=args.block, audio_seconds=args.audio_seconds, preparation_audio_seconds=2,
                          blocks=args.blocks, burn_in_ms=args.burn_in_ms, minimum_burn_in_cycles=1,
                          cooldown_seconds=args.cooldown_seconds, seed=seed, cases=names,
                          alternative="two-sided", alpha=ALPHA, correction="holm")
        blocks = run_blocks(names, lambda side, name: measure(binaries[side], name, args.rate, args.block,
                                                             args.audio_seconds, args.burn_in_ms),
                            args.blocks, args.cooldown_seconds, seed)
    results = analyze(blocks, names)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(dict(conditions=conditions, implementations=implementations,
                                          blocks=blocks, cases=results), indent=2, allow_nan=False) + "\n")
    print(f"Result: {args.output}")
    return report(results)


if __name__ == "__main__":
    sys.exit(main())

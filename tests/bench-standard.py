#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""Regression tests for randomized blocks, pairing and statistical reporting."""

from contextlib import redirect_stderr, redirect_stdout
import io
import math
from pathlib import Path
import random
import runpy
import unittest
from unittest.mock import patch

runner = runpy.run_path(str(Path(__file__).resolve().parent.parent / "scripts/bench-standard.py"))


def blocks():
    result = []
    for i in range(10):
        runs = []
        for case in ["first", "second"]:
            before = 100 + 20 * i
            for side, value in [("before", before), ("after", before + i + 1)]:
                runs.append(dict(case=case, side=side, ns=value, fingerprint="1234", peak=1, rms=0.5))
        random.Random(i).shuffle(runs)
        result.append(dict(number=i + 1, runs=runs))
    return result


class PairedTests(unittest.TestCase):
    def test_pairing_is_by_block_and_case_not_execution_order(self):
        result = runner["analyze"](blocks(), ["first", "second"])
        for row in result.values():
            self.assertEqual(row["blocks"], 10)
            self.assertEqual(row["df"], 9)
            self.assertEqual(row["differences_ns"], list(range(1, 11)))
            self.assertAlmostEqual(row["difference"]["mean"], 5.5)
            self.assertAlmostEqual(row["difference"]["sd"], math.sqrt(55 / 6))
            self.assertAlmostEqual(row["t"], math.sqrt(33), places=12)
            self.assertLess(row["p_holm"], 0.05)
            self.assertGreater(row["ci95"][0], 0)
            self.assertEqual(row["status"], "slower")

    def test_sign_and_nondetection(self):
        before = [100.0] * 10
        symmetric = [100 + i - 4.5 for i in range(10)]
        null = runner["paired_statistics"](before, symmetric)
        self.assertEqual(null["t"], 0)
        self.assertEqual(null["p"], 1)
        self.assertLess(null["ci95"][0], 0)
        self.assertGreater(null["ci95"][1], 0)
        slower = runner["paired_statistics"](before, list(range(101, 111)))
        faster = runner["paired_statistics"](list(range(101, 111)), before)
        self.assertEqual(slower["p"], faster["p"])
        self.assertEqual(slower["t"], -faster["t"])

    def test_degenerate_differences_do_not_claim_equivalence(self):
        for after in [[100.0] * 10, [101.0] * 10]:
            row = runner["paired_statistics"]([100.0] * 10, after)
            self.assertIsNone(row["t"])
            self.assertIsNone(row["p"])
            self.assertIsNone(row["ci95"])

    def test_holm_correction_and_undefined_family_member(self):
        self.assertEqual(runner["holm"]([0.04, 0.01, 0.03]), [0.06, 0.03, 0.06])
        self.assertEqual(runner["holm"]([None, 0.01, 0.04]), [None, 0.03, 0.08])
        # 未補正なら有意でも、比較族を考慮して有意と判定しない例。
        self.assertGreater(runner["holm"]([0.03, 0.04])[0], 0.05)

    def test_rejects_missing_duplicate_and_nonfinite_pairs(self):
        for data in [blocks()[:-1], blocks()]:
            if len(data) == 10:
                data[0]["runs"][0] = data[0]["runs"][1]
            with self.assertRaises(ValueError):
                runner["analyze"](data, ["first", "second"])
        with self.assertRaises(ValueError):
            runner["paired_statistics"]([100.0] * 10, [float("nan")] * 10)

    def test_shuffle_replay_coverage_and_cooldown(self):
        def execute(seed):
            now = [0]
            def sleep(seconds):
                now[0] += int(seconds * 1e9)
            def measure(side, name):
                now[0] += 3 * 10**9
                return dict(ns=100, burn_in_ns=3 * 10**9, fingerprint="1234", peak=1, rms=0.5)
            with redirect_stderr(io.StringIO()):
                return runner["run_blocks"](["first", "second"], measure, 10, 5, seed,
                                             clock=lambda: now[0], sleep=sleep)
        first, replay, other = execute(42), execute(42), execute(43)
        self.assertEqual(first, replay)
        self.assertNotEqual(first, other)
        for a, b in zip(first, first[1:]):
            self.assertEqual(b["started_ns"] - a["finished_ns"], 5 * 10**9)
        orders = set()
        for block in first:
            pairs = [(r["case"], r["side"]) for r in block["runs"]]
            self.assertEqual(set(pairs), {("first", "before"), ("first", "after"),
                                          ("second", "before"), ("second", "after")})
            self.assertEqual(len(pairs), 4)
            orders.add(tuple(pairs))
        self.assertGreater(len(orders), 1)

    def test_worker_burn_in_and_full_pass_are_validated(self):
        header = "case,ns,burn_in_ns,burn_in_cycles,measurement_ns,fingerprint,peak,rms\n"
        measure = runner["measure"]
        for ns, cycles, duration in [(9_999_999, 1, 19200000), (10_000_000, 0, 19200000),
                                      (10_000_000, 1, 0), (10_000_000, 1, 19200000)]:
            row = header + f"first,100,{ns},{cycles},{duration},1234,1,0.5\n"
            with patch.dict(measure.__globals__, capture=lambda *args: row):
                if ns < 10_000_000 or cycles < 1 or duration <= 0:
                    with self.assertRaises(ValueError):
                        measure(Path("unused"), "first", 48000, 256, 4, 10)
                else:
                    result = measure(Path("unused"), "first", 48000, 256, 4, 10)
                    self.assertEqual(result["ns"], 100)
                    self.assertEqual(result["measurement_ns"], 19200000)

    def test_report_does_not_reject_small_nonsignificant_increase(self):
        row = runner["paired_statistics"]([100.0] * 10, [100 + i - 4.4 for i in range(10)])
        row.update(p_holm=row["p"], status="not_detected", audio_same=True)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(runner["report"]({"case": row}), 0)
            row["status"] = "slower"
            self.assertEqual(runner["report"]({"case": row}), 1)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""Validate performance-trial collection without running timed DSP workloads."""

import csv
from contextlib import redirect_stderr, redirect_stdout
import io
from pathlib import Path
import runpy
import subprocess
import sys
import unittest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/bench-standard.py"
runner = runpy.run_path(str(SCRIPT))
collect_results = runner["collect_results"]
report_results = runner["report_results"]


def measurements(rounds=10):
    rows = []
    for trial in range(rounds):
        for case in ["first", "second"]:
            rows.append(dict(trial=trial + 1, started_ns=(30 + trial * 6) * 10**9,
                             finished_ns=(31 + trial * 6) * 10**9, case=case,
                             ns=trial + 1, fingerprint="1234", peak=1, rms=0.5))
    return rows


def collect(rows, rounds=10):
    stream = io.StringIO()
    writer = csv.DictWriter(stream, fieldnames=measurements()[0].keys())
    writer.writeheader()
    writer.writerows(rows)
    return collect_results(stream.getvalue(), ["first", "second"], rounds, 30, 5)


class TrialTests(unittest.TestCase):
    def test_even_and_odd_medians_keep_every_trial(self):
        for rounds, median in [(10, 5.5), (11, 6.0)]:
            cases, trials = collect(measurements(rounds), rounds)
            self.assertEqual(len(trials), rounds)
            for result in cases.values():
                self.assertEqual(result["samples_ns"], list(range(1, rounds + 1)))
                self.assertEqual(result["median_ns"], median)
                self.assertEqual(result["min_ns"], 1)
                self.assertEqual(result["max_ns"], rounds)

    def test_rejects_incomplete_early_or_inconsistent_trials(self):
        with self.assertRaisesRegex(ValueError, "Incomplete"):
            collect(measurements()[:-2])
        for row, key, value in [(0, "started_ns", 30 * 10**9 - 1),
                                (2, "started_ns", 36 * 10**9 - 1),
                                (1, "trial", 2), (1, "case", "first"),
                                (2, "fingerprint", "changed"), (0, "ns", "nan")]:
            with self.subTest(row=row, key=key):
                rows = measurements()
                rows[row][key] = value
                with self.assertRaises(ValueError):
                    collect(rows)

    def test_cli_rejects_settings_below_the_minimum(self):
        for option, value in [("--rounds", "9"), ("--burn-in-seconds", "29"),
                              ("--interval-seconds", "0")]:
            with self.subTest(option=option):
                result = subprocess.run([sys.executable, str(SCRIPT), option, value],
                                        capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, 2)
                self.assertIn(option, result.stderr)

    def test_any_median_increase_requires_review_without_offsetting_improvements(self):
        baseline, _ = collect(measurements())
        for worse, expected in [(5.5, 0), (5.500001, 1), (6.0, 1)]:
            candidate = {name: dict(row) for name, row in baseline.items()}
            candidate["first"]["median_ns"] = 1.0  # 大きな改善で別ケースの悪化を相殺しない。
            candidate["second"]["median_ns"] = worse
            with self.subTest(worse=worse), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(report_results(candidate, baseline), expected)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Prove the soak gates fail the build when a budget is exceeded.

These scripts were always correct; the workflow piped them into `tee`, which
reported the pipeline's status as `tee`'s and discarded theirs. The exit code
is therefore the contract worth pinning, not just the JSON body.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
COMPARE = ROOT / "scripts/compare_performance.py"
ABSOLUTE = ROOT / "scripts/check_soak_candidate.py"

# The real numbers recorded by the wasmtime/p2 soak, which reported success
# while the comparison it ran said `"passed": false`.
BASELINE = {
    "failures": 0,
    "first_byte_ms": {"p99": 88.05313700000283},
    "latency_ms": {"p99": 97.05890699996189},
    "requests_per_second": 2408.0909505934223,
    "rss_kib": {"start": 104664, "last_quarter_growth": 0},
}
CANDIDATE = {
    "failures": 0,
    "first_byte_ms": {"p99": 95.61899200002699},
    "latency_ms": {"p99": 105.36001699995268},
    "requests_per_second": 2248.735165217465,
    "rss_kib": {"start": 104664, "last_quarter_growth": -168},
}


def run(script: pathlib.Path, *args: str) -> tuple[int, dict]:
    """Run one gate and return its exit status with the parsed report."""
    completed = subprocess.run(
        [sys.executable, str(script), *args],
        capture_output=True,
        text=True,
        check=False,
    )
    return completed.returncode, json.loads(completed.stdout)


class SoakGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def write(self, name: str, payload: dict) -> str:
        path = pathlib.Path(self.directory.name) / name
        path.write_text(json.dumps(payload), encoding="utf-8")
        return str(path)

    def test_latency_regression_exits_non_zero(self) -> None:
        status, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", CANDIDATE),
        )
        self.assertEqual(status, 1)
        self.assertFalse(report["passed"])
        self.assertEqual(len(report["errors"]), 2)

    def test_matching_results_exit_zero(self) -> None:
        status, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", BASELINE),
        )
        self.assertEqual(status, 0)
        self.assertTrue(report["passed"])

    def test_throughput_is_reported_but_not_enforced_by_default(self) -> None:
        _, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", CANDIDATE),
        )
        throughput = report["throughput"]
        self.assertFalse(throughput["enforced"])
        self.assertLess(throughput["change_percent"], 0.0)
        self.assertFalse(
            [error for error in report["errors"] if "throughput" in error]
        )

    def test_latency_budget_is_configurable_per_lane(self) -> None:
        # The p2 lanes carry a measured ~8.5% gap against 0.3.2 and run with
        # --max-regression 0.12; the p3 lane baselines after that gap and keeps
        # the 5% default. Both behaviours have to hold on the same inputs, or
        # raising one lane's budget would quietly raise every lane's.
        arguments = (
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", CANDIDATE),
        )
        tolerant, report = run(
            COMPARE, *arguments, "--max-regression", "0.12"
        )
        self.assertEqual(tolerant, 0)
        self.assertTrue(report["passed"])

        strict, report = run(COMPARE, *arguments, "--max-regression", "0.05")
        self.assertEqual(strict, 1)
        self.assertFalse(report["passed"])

    def test_throughput_is_enforced_when_a_limit_is_supplied(self) -> None:
        status, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", CANDIDATE),
            "--max-throughput-regression",
            "0.05",
        )
        self.assertEqual(status, 1)
        self.assertTrue(report["throughput"]["enforced"])
        self.assertTrue(
            [error for error in report["errors"] if "throughput" in error]
        )

    def test_memory_allowance_takes_the_tighter_of_the_two_bounds(self) -> None:
        # 10% of a 104664 KiB start is far below the 32768 KiB ceiling, so the
        # proportional bound must win. Under the previous `max()` this growth
        # sat inside the ceiling and passed.
        leaking = dict(CANDIDATE)
        leaking["rss_kib"] = {"start": 104664, "last_quarter_growth": 20000}
        status, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", leaking),
        )
        self.assertEqual(status, 1)
        self.assertEqual(report["max_final_rss_growth_kib"], 10466)
        self.assertTrue(
            [error for error in report["errors"] if "RSS grew" in error]
        )

    def test_absolute_ceiling_stands_alone_without_a_start_sample(self) -> None:
        # Nothing to be proportional to, so the ceiling must not collapse to
        # zero and fail a healthy run.
        candidate = dict(CANDIDATE)
        candidate["rss_kib"] = {"last_quarter_growth": 1024}
        # A generous latency cap so this isolates the memory bound; the
        # default 25 ms would reject this candidate on p99 first.
        status, report = run(
            ABSOLUTE,
            self.write("candidate.json", candidate),
            "--max-p99-ms",
            "500",
        )
        self.assertEqual(status, 0)
        self.assertEqual(report["max_final_rss_growth_kib"], 32768)

    def test_absolute_gate_rejects_a_slow_candidate(self) -> None:
        status, report = run(
            ABSOLUTE, self.write("candidate.json", CANDIDATE), "--max-p99-ms", "25"
        )
        self.assertEqual(status, 1)
        self.assertFalse(report["passed"])

    def test_recorded_failures_fail_the_gate(self) -> None:
        candidate = dict(CANDIDATE)
        candidate["failures"] = 3
        status, _ = run(
            ABSOLUTE, self.write("candidate.json", candidate), "--max-p99-ms", "500"
        )
        self.assertEqual(status, 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)

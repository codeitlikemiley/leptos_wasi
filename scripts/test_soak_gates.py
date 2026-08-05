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
    "concurrency": 100,
    "achieved_concurrency": 99.9,
    "first_byte_ms": {"p99": 88.05313700000283},
    "latency_ms": {"p99": 97.05890699996189},
    "requests_per_second": 2408.0909505934223,
    "rss_kib": {"start": 104664, "last_quarter_growth": 0},
}
CANDIDATE = {
    "failures": 0,
    "concurrency": 100,
    "achieved_concurrency": 99.9,
    "first_byte_ms": {"p99": 95.61899200002699},
    "latency_ms": {"p99": 105.36001699995268},
    "requests_per_second": 2248.735165217465,
    "rss_kib": {"start": 104664, "last_quarter_growth": -168},
}
# The wasmtime/p2 candidate phase of run 30958088065, which reported 2478 rps
# with a 2.77 ms mean: 1487100 requests over 600 s never had more than ~6.9 in
# flight, so the probe was measuring itself. The rps landed mid-range of the
# legitimate 2312-3461 spread and the p99 sailed under every ceiling, which is
# why nothing caught it.
STARVED = {
    "failures": 0,
    "concurrency": 100,
    "achieved_concurrency": 6.865,
    "first_byte_ms": {"p99": 5.360000000000582},
    "latency_ms": {"p99": 6.006000000001222},
    "requests_per_second": 2478.249413594884,
    "rss_kib": {"start": 89516, "last_quarter_growth": -84},
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

    def test_a_starved_probe_passes_every_other_gate(self) -> None:
        # Establishes what the floor is for: on throughput, latency and memory
        # the starved run is not merely acceptable, it looks BETTER than a real
        # one. Nothing already in this file could have rejected it.
        status, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", STARVED),
            "--max-throughput-regression",
            "0.10",
        )
        self.assertEqual(status, 0)
        self.assertTrue(report["passed"])
        self.assertGreater(report["throughput"]["change_percent"], 0.0)

    def test_a_starved_candidate_is_rejected_as_unmeasured(self) -> None:
        status, report = run(
            COMPARE,
            self.write("baseline.json", BASELINE),
            self.write("candidate.json", STARVED),
            "--min-achieved-concurrency",
            "0.90",
        )
        self.assertEqual(status, 1)
        self.assertFalse(report["passed"])
        self.assertAlmostEqual(
            report["closed_loop"]["candidate"]["ratio_percent"], 6.865
        )
        self.assertTrue(
            [error for error in report["errors"] if "candidate reached only" in error]
        )

    def test_a_starved_baseline_misdiagnoses_rather_than_gates(self) -> None:
        # The other direction, and the one that is hardest to read. A baseline
        # that never applied its load records a low throughput, so the
        # candidate clears the throughput budget however slow it really was -
        # that gate is fooled outright. What fails instead is the latency
        # comparison, against a 6 ms p99 taken from an idle server, reporting a
        # four-figure "regression" in code that did not change. The build stops
        # for the wrong reason, which costs an investigation rather than
        # catching anything. The floor names it on the first line.
        arguments = (
            self.write("baseline.json", STARVED),
            self.write("candidate.json", CANDIDATE),
            "--max-throughput-regression",
            "0.10",
        )
        unguarded, report = run(COMPARE, *arguments)
        self.assertEqual(unguarded, 1)
        self.assertFalse(
            [error for error in report["errors"] if "throughput" in error]
        )
        self.assertTrue(
            [error for error in report["errors"] if "p99 regressed" in error]
        )

        guarded, report = run(
            COMPARE, *arguments, "--min-achieved-concurrency", "0.90"
        )
        self.assertEqual(guarded, 1)
        self.assertTrue(
            [error for error in report["errors"] if "baseline reached only" in error]
        )

    def test_the_absolute_gate_rejects_a_starved_candidate(self) -> None:
        # Its p99 of 6 ms clears the 150 ms ceiling by 25x precisely BECAUSE
        # the server was idle, so the absolute lane needs the same floor.
        status, report = run(
            ABSOLUTE,
            self.write("candidate.json", STARVED),
            "--max-p99-ms",
            "150",
            "--min-achieved-concurrency",
            "0.90",
        )
        self.assertEqual(status, 1)
        self.assertFalse(report["passed"])

    def test_the_floor_is_not_enforced_by_default(self) -> None:
        # Same contract as --max-throughput-regression: reported always, gated
        # only where a lane asks for it.
        status, report = run(
            ABSOLUTE, self.write("candidate.json", STARVED), "--max-p99-ms", "150"
        )
        self.assertEqual(status, 0)
        self.assertFalse(report["closed_loop"]["enforced"])
        self.assertAlmostEqual(report["closed_loop"]["ratio_percent"], 6.865)

    def test_a_run_without_the_field_cannot_satisfy_the_floor(self) -> None:
        # An older probe cannot be vouched for, and silently skipping the check
        # would make the floor unenforceable exactly when it is most needed.
        legacy = {key: value for key, value in CANDIDATE.items()}
        del legacy["achieved_concurrency"]
        status, report = run(
            ABSOLUTE,
            self.write("candidate.json", legacy),
            "--max-p99-ms",
            "500",
            "--min-achieved-concurrency",
            "0.90",
        )
        self.assertEqual(status, 1)
        self.assertIsNone(report["closed_loop"]["ratio_percent"])


if __name__ == "__main__":
    unittest.main(verbosity=2)

#!/usr/bin/env python3
"""Pin the soak-history formatter to CI comparison JSON."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/format-soak-history.py"

# Numbers from PR #37 Wasmtime P2 comparison JSON (run 34015218768).
WASM_P2 = {
    "passed": True,
    "errors": [],
    "comparisons": {
        "latency_ms": {
            "baseline_p99": 95.2878470000087,
            "candidate_p99": 83.1819299999097,
            "change_percent": -12.704576062147666,
        }
    },
    "throughput": {
        "baseline_requests_per_second": 2447.111742946329,
        "candidate_requests_per_second": 2757.2397801317934,
        "change_percent": 12.673227451888636,
    },
}
WASM_P3 = {
    "passed": True,
    "errors": [],
    "comparisons": {
        "latency_ms": {
            "baseline_p99": 81.379740,
            "candidate_p99": 81.426079,
            "change_percent": 0.0569,
        }
    },
    "throughput": {
        "baseline_requests_per_second": 2776.370812,
        "candidate_requests_per_second": 2779.094765,
        "change_percent": 0.0981,
    },
}
SPIN_P2 = {
    "passed": True,
    "errors": [],
    "comparisons": {
        "latency_ms": {
            "baseline_p99": 92.535634,
            "candidate_p99": 97.168611,
            "change_percent": 5.0067,
        }
    },
    "throughput": {
        "baseline_requests_per_second": 2496.801668,
        "candidate_requests_per_second": 2395.659490,
        "change_percent": -4.0509,
    },
}


def run(*args: str, check: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        capture_output=True,
        text=True,
        check=check,
    )


class FormatSoakHistoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)

    def write(self, name: str, payload: dict) -> str:
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload), encoding="utf-8")
        return str(path)

    def test_formats_a_wasmtime_p2_row_from_pr_37_numbers(self) -> None:
        path = self.write("soak-wasmtime-p2-comparison.json", WASM_P2)
        completed = run(
            "--date",
            "2026-09-06",
            "--sha",
            "131f7b681cada5353fb8f13c4a91509e84ea8378",
            "--pr",
            "37",
            "--run",
            "34015218768",
            "--reuse",
            "wasmtime/p2=128",
            "--notes",
            "PR soak on head",
            path,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            completed.stdout,
            "| 2026-09-06 | 131f7b6 (#37) | Wasmtime P2 | 128 | 9689c68 | "
            "2447.11 to 2757.24 | 95.29 to 83.18 | +12.67% | -12.70% | pass | "
            "CI run 34015218768; PR soak on head |\n",
        )

    def test_header_precedes_the_row(self) -> None:
        path = self.write("soak-wasmtime-p2-comparison.json", WASM_P2)
        completed = run("--header", path)
        lines = completed.stdout.splitlines()
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertTrue(lines[0].startswith("| Date | Commit | Lane |"))
        self.assertTrue(lines[1].startswith("|---"))
        self.assertIn("Wasmtime P2", lines[2])

    def test_spin_defaults_reuse_to_na_and_wasmtime_stays_blank(self) -> None:
        spin = self.write("soak-spin-p2-comparison.json", SPIN_P2)
        wasm = self.write("soak-wasmtime-p3-comparison.json", WASM_P3)
        completed = run(spin, wasm)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        lines = completed.stdout.splitlines()
        self.assertIn("| Spin P2 | n/a | 9689c68 |", lines[0])
        self.assertIn("| Wasmtime P3 | — | 663e1a9 |", lines[1])

    def test_directory_emits_lanes_in_matrix_order(self) -> None:
        nested = self.root / "artifacts"
        self.write("artifacts/soak-spin-p2/soak-spin-p2-comparison.json", SPIN_P2)
        self.write(
            "artifacts/soak-wasmtime-p3/soak-wasmtime-p3-comparison.json", WASM_P3
        )
        self.write(
            "artifacts/soak-wasmtime-p2/soak-wasmtime-p2-comparison.json", WASM_P2
        )
        completed = run(str(nested))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        lanes = [
            line.split("|")[3].strip() for line in completed.stdout.splitlines()
        ]
        self.assertEqual(lanes, ["Wasmtime P2", "Wasmtime P3", "Spin P2"])

    def test_absolute_failure_fails_the_gate_column(self) -> None:
        path = self.write("soak-wasmtime-p2-comparison.json", WASM_P2)
        self.write(
            "soak-wasmtime-p2-absolute.json",
            {"passed": False, "errors": ["p99 too high"]},
        )
        completed = run(path)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("| fail |", completed.stdout)

    def test_comparison_failure_fails_the_gate_column(self) -> None:
        failing = dict(WASM_P2)
        failing["passed"] = False
        path = self.write("soak-wasmtime-p2-comparison.json", failing)
        completed = run(path)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("| fail |", completed.stdout)

    def test_rejects_a_pipe_in_notes(self) -> None:
        path = self.write("soak-wasmtime-p2-comparison.json", WASM_P2)
        completed = run("--notes", "a | b", path)
        self.assertEqual(completed.returncode, 1)
        self.assertIn("'|'", completed.stderr)

    def test_rejects_unknown_filenames(self) -> None:
        path = self.write("result.json", WASM_P2)
        completed = run(path)
        self.assertEqual(completed.returncode, 1)
        self.assertIn("not a soak comparison file", completed.stderr)

    def test_reuse_values_may_contain_spaces(self) -> None:
        path = self.write("soak-wasmtime-p3-comparison.json", WASM_P3)
        completed = run("--reuse", "wasmtime/p3=host default", path)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("| Wasmtime P3 | host default |", completed.stdout)

    def test_override_baseline_ref(self) -> None:
        path = self.write("soak-wasmtime-p2-comparison.json", WASM_P2)
        completed = run("--baseline-ref", "wasmtime/p2=deadbeef", path)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("| deadbeef |", completed.stdout)

    def test_empty_directory_is_an_error(self) -> None:
        completed = run(str(self.root))
        self.assertEqual(completed.returncode, 1)
        self.assertIn("no soak-*-comparison.json", completed.stderr)

    def test_missing_latency_block_is_an_error(self) -> None:
        path = self.write(
            "soak-wasmtime-p2-comparison.json",
            {"passed": True, "throughput": WASM_P2["throughput"]},
        )
        completed = run(path)
        self.assertEqual(completed.returncode, 1)
        self.assertIn("comparisons", completed.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)

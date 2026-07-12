#!/usr/bin/env python3
"""Tests for the trusted-load soak promotion gate."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-trusted-load-soak.py")
SPEC = importlib.util.spec_from_file_location("check_trusted_load_soak", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def fixture() -> dict:
    samples = [
        {"alive": True, "rss_kib": 10_000},
        {"alive": True, "rss_kib": 10_250},
        {"alive": True, "rss_kib": 10_500},
        {"alive": True, "rss_kib": 10_750},
        {"alive": True, "rss_kib": 11_000},
        {"alive": True, "rss_kib": 11_250},
        {"alive": True, "rss_kib": 11_500},
        {"alive": True, "rss_kib": 12_000},
    ]
    return {
        "schema": 2,
        "configuration": {
            "mode": "closed-loop",
            "duration_seconds": 600.0,
            "elapsed_seconds": 600.1,
            "concurrency": 100,
        },
        "totals": {
            "completed": 1_000,
            "status_failures": 0,
            "transport_failures": 0,
            "canceled": 0,
            "hung": 0,
        },
        "latency_ms": {"successful": {"p99": 20.0}},
        "response_headers_ms": {"p99": 19.0},
        "first_body_byte_ms": {"p99": 19.5},
        "processes": {"ingress": samples},
    }


class SoakGateTests(unittest.TestCase):
    def test_passing_report_should_preserve_gate_evidence(self) -> None:
        summary = MODULE.summarize(fixture())

        self.assertTrue(summary["passed"])
        self.assertEqual(summary["errors"], [])
        self.assertEqual(
            summary["processes"]["ingress"]["final_quarter_growth_kib"],
            500,
        )

    def test_failures_and_latency_should_block_promotion(self) -> None:
        report = fixture()
        report["totals"]["status_failures"] = 3
        report["latency_ms"]["successful"]["p99"] = 30.0

        summary = MODULE.summarize(report)

        self.assertFalse(summary["passed"])
        self.assertTrue(
            any("status_failures" in error for error in summary["errors"])
        )
        self.assertTrue(any("p99" in error for error in summary["errors"]))

    def test_truncated_or_growing_soak_should_block_promotion(self) -> None:
        report = fixture()
        report["configuration"]["elapsed_seconds"] = 30.0
        report["processes"]["ingress"][-1]["rss_kib"] = 50_000

        summary = MODULE.summarize(report)

        self.assertFalse(summary["passed"])
        self.assertTrue(any("ended after" in error for error in summary["errors"]))
        self.assertTrue(any("RSS grew" in error for error in summary["errors"]))


if __name__ == "__main__":
    unittest.main()

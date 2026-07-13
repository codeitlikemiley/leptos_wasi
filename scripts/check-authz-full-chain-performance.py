#!/usr/bin/env python3
"""Enforce the local full authentication/authorization chain p99 budget."""

from __future__ import annotations

import json
import os
import pathlib
import sys


MAX_P99_MS = 25.0


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: check-authz-full-chain-performance.py INPUT OUTPUT")
    source = pathlib.Path(sys.argv[1])
    output = pathlib.Path(sys.argv[2])
    result = json.loads(source.read_text(encoding="utf-8"))
    summary = result.get("summary", {})
    statuses = result.get("statusCodeDistribution", {})
    errors = result.get("errorDistribution", {})
    latency = result.get("latencyPercentiles", {})
    first_byte = result.get("firstBytePercentiles", {})
    total_p99_ms = float(latency.get("p99", float("inf"))) * 1000.0
    first_byte_p99_ms = float(first_byte.get("p99", float("inf"))) * 1000.0
    failures = sum(int(count) for count in errors.values())
    failures += sum(
        int(count)
        for status, count in statuses.items()
        if not 200 <= int(status) < 300
    )
    if summary.get("successRate") != 1.0:
        failures = max(failures, 1)
    report = {
        "concurrency": int(os.environ.get("AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY", "100")),
        "requested_rate": os.environ.get("AUTHZ_FULL_CHAIN_BENCHMARK_RATE"),
        "max_p99_ms": MAX_P99_MS,
        "requests_per_second": summary.get("requestsPerSec"),
        "first_byte_p99_ms": first_byte_p99_ms,
        "latency_p99_ms": total_p99_ms,
        "failures": failures,
        "passed": (
            failures == 0
            and first_byte_p99_ms <= MAX_P99_MS
            and total_p99_ms <= MAX_P99_MS
        ),
    }
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output.read_text(encoding="utf-8"), end="")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

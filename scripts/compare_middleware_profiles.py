#!/usr/bin/env python3
"""Compare repeated unwrapped and fused Leptos runtime probes."""

from __future__ import annotations

import argparse
import json
import statistics
from pathlib import Path


def load(path: Path) -> dict[str, object]:
    with path.open(encoding="utf-8") as source:
        value = json.load(source)
    if "summary" in value:
        value = normalize_oha(path, value)
    if value.get("failures") != 0:
        raise ValueError(f"{path} contains failed requests")
    return value


def normalize_oha(path: Path, value: dict[str, object]) -> dict[str, object]:
    """Translate pinned oha JSON into the runtime probe metric shape."""
    summary = value.get("summary")
    latency = value.get("latencyPercentiles")
    first_byte = value.get("firstBytePercentiles")
    statuses = value.get("statusCodeDistribution")
    errors = value.get("errorDistribution")
    if not all(
        isinstance(group, dict)
        for group in (summary, latency, first_byte, statuses, errors)
    ):
        raise ValueError(f"{path} is missing required oha metric groups")

    assert isinstance(summary, dict)
    assert isinstance(latency, dict)
    assert isinstance(first_byte, dict)
    assert isinstance(statuses, dict)
    assert isinstance(errors, dict)
    success_rate = summary.get("successRate")
    requests_per_second = summary.get("requestsPerSec")
    latency_p99 = latency.get("p99")
    first_byte_p99 = first_byte.get("p99")
    if not all(
        isinstance(metric_value, (int, float))
        for metric_value in (
            success_rate,
            requests_per_second,
            latency_p99,
            first_byte_p99,
        )
    ):
        raise ValueError(f"{path} contains non-numeric oha metrics")

    unexpected_statuses = sum(
        int(count)
        for status, count in statuses.items()
        if not 200 <= int(status) < 400
    )
    transport_errors = sum(int(count) for count in errors.values())
    failures = unexpected_statuses + transport_errors
    if success_rate != 1.0:
        failures = max(failures, 1)
    return {
        "failures": failures,
        "requests_per_second": requests_per_second,
        # oha reports durations in seconds; the existing comparison output is
        # deliberately kept in milliseconds.
        "latency_ms": {"p99": float(latency_p99) * 1000.0},
        "first_byte_ms": {"p99": float(first_byte_p99) * 1000.0},
    }


def metric(result: dict[str, object], group: str, name: str) -> float:
    value: object
    if group:
        nested = result.get(group)
        if not isinstance(nested, dict):
            raise ValueError(f"missing {group} metric group")
        value = nested.get(name)
    else:
        value = result.get(name)
    if not isinstance(value, (int, float)):
        raise ValueError(f"missing numeric metric {group}.{name}")
    return float(value)


def median_metric(
    results: list[dict[str, object]], group: str, name: str
) -> float:
    return statistics.median(metric(result, group, name) for result in results)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", action="append", type=Path, required=True)
    parser.add_argument("--candidate", action="append", type=Path, required=True)
    parser.add_argument("--baseline-label", default="unwrapped")
    parser.add_argument("--candidate-label", default="fused")
    parser.add_argument("--workload", default="unspecified")
    parser.add_argument("--max-regression", type=float, default=0.10)
    args = parser.parse_args()

    if len(args.baseline) != len(args.candidate) or len(args.baseline) < 5:
        raise SystemExit("at least five paired baseline/candidate probes are required")

    try:
        baseline = [load(path) for path in args.baseline]
        candidate = [load(path) for path in args.candidate]
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(str(error)) from error

    errors: list[str] = []
    comparisons: dict[str, dict[str, float]] = {}
    for label, group, name in (
        ("first_byte_p99_ms", "first_byte_ms", "p99"),
        ("latency_p99_ms", "latency_ms", "p99"),
    ):
        before = median_metric(baseline, group, name)
        after = median_metric(candidate, group, name)
        change = (after / before) - 1.0 if before else 0.0
        comparisons[label] = {
            "baseline_median": before,
            "candidate_median": after,
            "change_percent": change * 100.0,
        }
        if after > before * (1.0 + args.max_regression):
            errors.append(
                f"{label} regressed {change * 100.0:.2f}% "
                f"(allowed {args.max_regression * 100.0:.2f}%)"
            )

    before_rps = median_metric(baseline, "", "requests_per_second")
    after_rps = median_metric(candidate, "", "requests_per_second")
    throughput_change = (after_rps / before_rps) - 1.0 if before_rps else 0.0
    comparisons["throughput_requests_per_second"] = {
        "baseline_median": before_rps,
        "candidate_median": after_rps,
        "change_percent": throughput_change * 100.0,
    }
    if after_rps < before_rps * (1.0 - args.max_regression):
        errors.append(
            f"throughput regressed {-throughput_change * 100.0:.2f}% "
            f"(allowed {args.max_regression * 100.0:.2f}%)"
        )

    summary = {
        "runs": len(baseline),
        "baseline": args.baseline_label,
        "candidate": args.candidate_label,
        "workload": args.workload,
        "max_regression_percent": args.max_regression * 100.0,
        "comparisons": comparisons,
        "passed": not errors,
        "errors": errors,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Compare a candidate soak result with its retained baseline."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def read_result(path: Path) -> dict[str, object]:
    with path.open(encoding="utf-8") as source:
        return json.load(source)


def nested_number(result: dict[str, object], group: str, field: str) -> float:
    values = result.get(group)
    if not isinstance(values, dict):
        raise ValueError(f"{group!r} is missing from the probe result")
    value = values.get(field)
    if not isinstance(value, (int, float)):
        raise ValueError(f"{group}.{field} is not numeric")
    return float(value)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.05)
    parser.add_argument("--max-final-rss-growth-kib", type=int, default=32768)
    args = parser.parse_args()

    baseline = read_result(args.baseline)
    candidate = read_result(args.candidate)
    failures = candidate.get("failures")
    if failures != 0:
        raise SystemExit(f"candidate recorded {failures!r} failed requests")

    comparisons: dict[str, dict[str, float]] = {}
    errors: list[str] = []
    for group in ("first_byte_ms", "latency_ms"):
        baseline_p99 = nested_number(baseline, group, "p99")
        candidate_p99 = nested_number(candidate, group, "p99")
        limit = baseline_p99 * (1.0 + args.max_regression)
        change = (
            (candidate_p99 / baseline_p99) - 1.0
            if baseline_p99
            else 0.0
        )
        comparisons[group] = {
            "baseline_p99": baseline_p99,
            "candidate_p99": candidate_p99,
            "change_percent": change * 100.0,
            "allowed_p99": limit,
        }
        if candidate_p99 > limit:
            errors.append(
                f"{group} p99 regressed {change * 100.0:.2f}% "
                f"(allowed {args.max_regression * 100.0:.2f}%)"
            )

    baseline_rps = float(baseline.get("requests_per_second", 0.0))
    candidate_rps = float(candidate.get("requests_per_second", 0.0))
    throughput_change = (
        (candidate_rps / baseline_rps) - 1.0 if baseline_rps else 0.0
    )

    rss = candidate.get("rss_kib")
    if not isinstance(rss, dict):
        errors.append("candidate RSS samples are missing")
        final_rss_growth = None
    else:
        final_rss_growth = rss.get("last_quarter_growth")
        if not isinstance(final_rss_growth, int):
            errors.append("candidate final-quarter RSS growth is unavailable")
        else:
            rss_start = rss.get("start")
            proportional_limit = (
                int(float(rss_start) * 0.10)
                if isinstance(rss_start, (int, float))
                else 0
            )
            memory_limit = max(args.max_final_rss_growth_kib, proportional_limit)
        if isinstance(final_rss_growth, int) and final_rss_growth > memory_limit:
            errors.append(
                "candidate RSS grew "
                f"{final_rss_growth} KiB in the final quarter "
                f"(allowed {memory_limit} KiB)"
            )

    summary = {
        "baseline": str(args.baseline),
        "candidate": str(args.candidate),
        "comparisons": comparisons,
        "throughput": {
            "baseline_requests_per_second": baseline_rps,
            "candidate_requests_per_second": candidate_rps,
            "change_percent": throughput_change * 100.0,
        },
        "candidate_final_quarter_rss_growth_kib": final_rss_growth,
        "passed": not errors,
        "errors": errors,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())

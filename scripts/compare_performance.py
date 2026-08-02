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
    # Reported always, enforced only when a limit is supplied. The sibling
    # `compare_middleware_profiles.py` gates throughput at 10%; this lane has
    # no measured run-to-run spread yet, so the number is collected first and
    # the limit is set from evidence rather than guessed.
    parser.add_argument("--max-throughput-regression", type=float, default=None)
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
    throughput_limit = args.max_throughput_regression
    if throughput_limit is not None and baseline_rps:
        if candidate_rps < baseline_rps * (1.0 - throughput_limit):
            errors.append(
                f"throughput regressed {-throughput_change * 100.0:.2f}% "
                f"(allowed {throughput_limit * 100.0:.2f}%)"
            )

    rss = candidate.get("rss_kib")
    memory_limit = args.max_final_rss_growth_kib
    if not isinstance(rss, dict):
        errors.append("candidate RSS samples are missing")
        final_rss_growth = None
    else:
        final_rss_growth = rss.get("last_quarter_growth")
        if not isinstance(final_rss_growth, int):
            errors.append("candidate final-quarter RSS growth is unavailable")
        else:
            rss_start = rss.get("start")
            # Whichever allowance is TIGHTER. A small process would never
            # reach the absolute ceiling, so the proportional bound is what
            # makes a small leak visible. Without a starting sample there is
            # nothing to be proportional to, so the ceiling stands alone.
            if isinstance(rss_start, (int, float)) and rss_start > 0:
                memory_limit = min(
                    memory_limit, int(float(rss_start) * 0.10)
                )
            if final_rss_growth > memory_limit:
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
            "enforced": throughput_limit is not None,
            "max_regression_percent": (
                None
                if throughput_limit is None
                else throughput_limit * 100.0
            ),
        },
        "candidate_final_quarter_rss_growth_kib": final_rss_growth,
        "max_final_rss_growth_kib": memory_limit,
        "passed": not errors,
        "errors": errors,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())

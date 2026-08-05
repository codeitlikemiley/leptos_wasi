#!/usr/bin/env python3
"""Enforce absolute soak gates when no compatible historical baseline exists."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def nested_number(result: dict[str, object], group: str, field: str) -> float:
    """Read one required numeric field from a nested probe result."""
    values = result.get(group)
    if not isinstance(values, dict):
        raise ValueError(f"{group!r} is missing from the probe result")
    value = values.get(field)
    if not isinstance(value, (int, float)):
        raise ValueError(f"{group}.{field} is not numeric")
    return float(value)


def closed_loop_report(
    result: dict[str, object],
    minimum: float | None,
    errors: list[str],
) -> dict[str, object]:
    """Record - and optionally gate - how closed the closed loop actually was.

    A probe that never reached its requested concurrency measured its own
    ceiling rather than the server's. Its latency percentiles then describe a
    barely-loaded server and sail under any absolute ceiling, so a run that
    failed to apply the load looks healthier here than one that did.
    """
    requested = result.get("concurrency")
    achieved = result.get("achieved_concurrency")
    ratio: float | None = None
    if (
        isinstance(requested, (int, float))
        and requested > 0
        and isinstance(achieved, (int, float))
    ):
        ratio = float(achieved) / float(requested)
    if minimum is not None:
        if ratio is None:
            errors.append("candidate run does not report achieved concurrency")
        elif ratio < minimum:
            errors.append(
                f"candidate reached only {ratio * 100.0:.1f}% of its requested "
                f"concurrency (required {minimum * 100.0:.1f}%); the probe was "
                "the bottleneck, so this run does not measure the server"
            )
    return {
        "requested": requested if isinstance(requested, (int, float)) else None,
        "achieved": achieved if isinstance(achieved, (int, float)) else None,
        "ratio_percent": None if ratio is None else ratio * 100.0,
        "enforced": minimum is not None,
        "minimum_percent": None if minimum is None else minimum * 100.0,
    }


def main() -> int:
    """Validate correctness, latency, and terminal memory growth."""
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--max-p99-ms", type=float, default=25.0)
    parser.add_argument("--max-final-rss-growth-kib", type=int, default=32768)
    # Reported always, enforced only when a floor is supplied.
    parser.add_argument("--min-achieved-concurrency", type=float, default=None)
    args = parser.parse_args()

    with args.candidate.open(encoding="utf-8") as source:
        candidate = json.load(source)

    errors: list[str] = []
    failures = candidate.get("failures")
    if failures != 0:
        errors.append(f"candidate recorded {failures!r} failed requests")

    closed_loop = closed_loop_report(
        candidate, args.min_achieved_concurrency, errors
    )

    p99_values = {
        group: nested_number(candidate, group, "p99")
        for group in ("first_byte_ms", "latency_ms")
    }
    for group, p99 in p99_values.items():
        if p99 > args.max_p99_ms:
            errors.append(
                f"{group} p99 was {p99:.3f} ms "
                f"(allowed {args.max_p99_ms:.3f} ms)"
            )

    rss = candidate.get("rss_kib")
    final_rss_growth: int | None = None
    memory_limit = args.max_final_rss_growth_kib
    if not isinstance(rss, dict):
        errors.append("candidate RSS samples are missing")
    else:
        value = rss.get("last_quarter_growth")
        if not isinstance(value, int):
            errors.append("candidate final-quarter RSS growth is unavailable")
        else:
            final_rss_growth = value
            rss_start = rss.get("start")
            # Hold a process to whichever allowance is TIGHTER: the absolute
            # ceiling, or a proportion of where it started. A small process
            # would never reach the absolute ceiling, so the proportional
            # bound is what makes a small leak visible. When the starting
            # sample is unavailable there is nothing to be proportional to,
            # so the absolute ceiling stands alone.
            if isinstance(rss_start, (int, float)) and rss_start > 0:
                memory_limit = min(memory_limit, int(float(rss_start) * 0.10))
            if final_rss_growth > memory_limit:
                errors.append(
                    f"candidate RSS grew {final_rss_growth} KiB in the final quarter "
                    f"(allowed {memory_limit} KiB)"
                )

    summary = {
        "candidate": str(args.candidate),
        "closed_loop": closed_loop,
        "failures": failures,
        "p99_ms": p99_values,
        "max_p99_ms": args.max_p99_ms,
        "candidate_final_quarter_rss_growth_kib": final_rss_growth,
        "max_final_rss_growth_kib": memory_limit,
        "passed": not errors,
        "errors": errors,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())

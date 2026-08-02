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


def main() -> int:
    """Validate correctness, latency, and terminal memory growth."""
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--max-p99-ms", type=float, default=25.0)
    parser.add_argument("--max-final-rss-growth-kib", type=int, default=32768)
    args = parser.parse_args()

    with args.candidate.open(encoding="utf-8") as source:
        candidate = json.load(source)

    errors: list[str] = []
    failures = candidate.get("failures")
    if failures != 0:
        errors.append(f"candidate recorded {failures!r} failed requests")

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

#!/usr/bin/env python3
"""Summarize and enforce the trusted-ingress production soak gate."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def numeric(value: object, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} must be numeric")
    return float(value)


def integer(value: object, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"{label} must be an integer")
    return value


def summarize(
    report: dict[str, Any],
    *,
    max_p99_ms: float = 25.0,
    max_rss_growth_kib: int = 32_768,
    max_rss_growth_fraction: float = 0.10,
) -> dict[str, Any]:
    errors: list[str] = []
    configuration = report.get("configuration")
    totals = report.get("totals")
    if report.get("schema") != 2:
        errors.append("trusted-load report schema must be 2")
    if not isinstance(configuration, dict):
        raise ValueError("configuration must be an object")
    if not isinstance(totals, dict):
        raise ValueError("totals must be an object")

    mode = configuration.get("mode")
    configured_duration = numeric(
        configuration.get("duration_seconds"),
        "configuration.duration_seconds",
    )
    elapsed = numeric(
        configuration.get("elapsed_seconds"),
        "configuration.elapsed_seconds",
    )
    if mode != "closed-loop":
        errors.append("soak report must use closed-loop mode")
    if elapsed < configured_duration:
        errors.append(
            f"soak ended after {elapsed:.3f}s before the configured "
            f"{configured_duration:.3f}s duration"
        )

    failure_fields = (
        "status_failures",
        "transport_failures",
        "canceled",
        "hung",
    )
    failures = {
        field: integer(totals.get(field), f"totals.{field}")
        for field in failure_fields
    }
    for field, count in failures.items():
        if count:
            errors.append(f"totals.{field} recorded {count} failures")

    completed = integer(totals.get("completed"), "totals.completed")
    requests_per_second = completed / elapsed if elapsed else 0.0
    latency_groups = {
        "successful_total": report.get("latency_ms", {}).get("successful"),
        "response_headers": report.get("response_headers_ms"),
        "first_body_byte": report.get("first_body_byte_ms"),
    }
    latency_summary: dict[str, dict[str, object]] = {}
    for name, values in latency_groups.items():
        if not isinstance(values, dict):
            raise ValueError(f"{name} latency summary must be an object")
        p99_ms = numeric(values.get("p99"), f"{name}.p99")
        passed = p99_ms <= max_p99_ms
        latency_summary[name] = {
            "p99_ms": p99_ms,
            "max_p99_ms": max_p99_ms,
            "passed": passed,
        }
        if not passed:
            errors.append(
                f"{name} p99 {p99_ms:.3f}ms exceeds {max_p99_ms:.3f}ms"
            )

    process_values = report.get("processes")
    if not isinstance(process_values, dict) or not process_values:
        raise ValueError("processes must be a non-empty object")
    process_summary: dict[str, dict[str, object]] = {}
    for name, samples in sorted(process_values.items()):
        if not isinstance(samples, list) or len(samples) < 4:
            errors.append(f"process {name} has fewer than four samples")
            continue
        if not isinstance(samples[-1], dict) or samples[-1].get("alive") is not True:
            errors.append(f"process {name} was not alive at the final sample")
        rss = [
            sample.get("rss_kib")
            for sample in samples
            if isinstance(sample, dict)
            and isinstance(sample.get("rss_kib"), int)
            and not isinstance(sample.get("rss_kib"), bool)
        ]
        if len(rss) < 4:
            errors.append(f"process {name} has fewer than four RSS samples")
            continue
        quarter_start = rss[len(rss) * 3 // 4]
        growth = rss[-1] - quarter_start
        allowed_growth = max(
            max_rss_growth_kib,
            int(rss[0] * max_rss_growth_fraction),
        )
        passed = growth <= allowed_growth
        process_summary[str(name)] = {
            "samples": len(rss),
            "start_rss_kib": rss[0],
            "end_rss_kib": rss[-1],
            "max_rss_kib": max(rss),
            "final_quarter_start_rss_kib": quarter_start,
            "final_quarter_growth_kib": growth,
            "allowed_final_quarter_growth_kib": allowed_growth,
            "passed": passed,
        }
        if not passed:
            errors.append(
                f"process {name} final-quarter RSS grew {growth} KiB "
                f"(allowed {allowed_growth} KiB)"
            )

    return {
        "schema": 1,
        "configuration": {
            "mode": mode,
            "configured_duration_seconds": configured_duration,
            "elapsed_seconds": elapsed,
            "concurrency": configuration.get("concurrency"),
        },
        "totals": {
            "completed": completed,
            **failures,
            "requests_per_second": requests_per_second,
        },
        "latency": latency_summary,
        "processes": process_summary,
        "passed": not errors,
        "errors": errors,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--max-p99-ms", type=float, default=25.0)
    parser.add_argument("--max-rss-growth-kib", type=int, default=32_768)
    parser.add_argument("--max-rss-growth-fraction", type=float, default=0.10)
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding="utf-8"))
    summary = summarize(
        report,
        max_p99_ms=args.max_p99_ms,
        max_rss_growth_kib=args.max_rss_growth_kib,
        max_rss_growth_fraction=args.max_rss_growth_fraction,
    )
    rendered = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        print(rendered, end="")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
        print(args.output)
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

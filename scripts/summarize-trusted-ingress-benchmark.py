#!/usr/bin/env python3
"""Summarize and enforce repeated trusted-ingress benchmark reports."""
import json
import pathlib
import statistics
import sys

root = pathlib.Path(sys.argv[1])
summary = {}
passed = True
reports_by_profile = {}

for profile in sorted(path for path in root.iterdir() if path.is_dir()):
    reports = []
    for path in sorted(profile.glob("*/result.json"), key=lambda value: int(value.parent.name)):
        report = json.loads(path.read_text())
        elapsed = report["configuration"]["elapsed_seconds"]
        totals = report["totals"]
        failures = (
            totals["status_failures"]
            + totals["transport_failures"]
            + totals["canceled"]
            + totals["hung"]
        )
        reports.append({
            "repetition": int(path.parent.name),
            "p99_ms": report["latency_ms"]["successful"]["p99"],
            "first_byte_p99_ms": report["first_byte_ms"]["p99"],
            "requests_per_second": totals["completed"] / elapsed,
            "failures": failures,
        })
    if not reports:
        continue
    reports_by_profile[profile.name] = reports
    profile_passed = all(
        report["p99_ms"] <= 25
        and report["first_byte_p99_ms"] <= 25
        and report["failures"] == 0
        for report in reports
    )
    passed &= profile_passed
    summary[profile.name] = {
        "repetitions": len(reports),
        "passed": profile_passed,
        "worst_p99_ms": max(report["p99_ms"] for report in reports),
        "worst_first_byte_p99_ms": max(report["first_byte_p99_ms"] for report in reports),
        "p99_ms_median": statistics.median(report["p99_ms"] for report in reports),
        "requests_per_second_median": statistics.median(report["requests_per_second"] for report in reports),
        "failures": [report["failures"] for report in reports],
    }

baselines = {report["repetition"]: report for report in reports_by_profile.get("proxy-baseline", [])}
edges = {report["repetition"]: report for report in reports_by_profile.get("edge-anonymous", [])}
pairs = []
for repetition in sorted(set(baselines) & set(edges)):
    baseline = baselines[repetition]
    edge = edges[repetition]
    latency_regression = edge["p99_ms"] / baseline["p99_ms"] - 1 if baseline["p99_ms"] else 0
    throughput_regression = 1 - edge["requests_per_second"] / baseline["requests_per_second"]
    pair_passed = latency_regression <= 0.10 and throughput_regression <= 0.10
    passed &= pair_passed
    pairs.append({
        "repetition": repetition,
        "latency_regression": latency_regression,
        "throughput_regression": throughput_regression,
        "passed": pair_passed,
    })

summary["edge_pairs"] = pairs
summary["promotion_passed"] = passed and bool(pairs)
output = root / "summary.json"
output.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
print(output)
if not summary["promotion_passed"]:
    raise SystemExit(1)

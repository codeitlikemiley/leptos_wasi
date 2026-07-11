#!/usr/bin/env python3
"""Summarize repeated trusted-ingress benchmark JSON reports."""
import json
import pathlib
import statistics
import sys

root = pathlib.Path(sys.argv[1])
summary = {}
for profile in sorted(path for path in root.iterdir() if path.is_dir()):
    reports = [json.loads(path.read_text()) for path in profile.glob("*/summary.json")]
    if not reports:
        continue
    summary[profile.name] = {
        "repetitions": len(reports),
        "p99_ms_median": statistics.median(report["latency_p99_ms"] for report in reports),
        "first_byte_p99_ms_median": statistics.median(report["first_byte_p99_ms"] for report in reports),
        "requests_per_second_median": statistics.median(report["requests_per_second"] for report in reports),
        "failures": [report.get("failures", 0) for report in reports],
    }
output = root / "summary.json"
output.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
print(output)

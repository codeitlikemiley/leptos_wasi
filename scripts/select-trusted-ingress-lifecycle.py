#!/usr/bin/env python3
"""Select the valid Wasmtime lifecycle candidate with the best worst p99."""
import json
import pathlib
import statistics
import sys

root = pathlib.Path(sys.argv[1])
repetitions = int(sys.argv[2])
concurrencies = [int(value) for value in sys.argv[3:]]
candidates = []

for candidate in sorted(path for path in root.glob("reuse-*-concurrent-*") if path.is_dir()):
    reports = []
    valid = True
    for concurrency in concurrencies:
        for repetition in range(1, repetitions + 1):
            directory = candidate / f"concurrency-{concurrency}" / str(repetition)
            result = directory / "result.json"
            if (directory / "rejected").exists() or not result.exists():
                valid = False
                continue
            report = json.loads(result.read_text())
            totals = report["totals"]
            failures = sum(totals[key] for key in (
                "status_failures", "transport_failures", "canceled", "hung"
            ))
            if failures:
                valid = False
            reports.append(report)
    if not valid or len(reports) != repetitions * len(concurrencies):
        continue
    worst_p99 = max(report["latency_ms"]["successful"]["p99"] for report in reports)
    throughput = statistics.median(
        report["totals"]["completed"] / report["configuration"]["elapsed_seconds"]
        for report in reports
    )
    candidates.append((worst_p99, -throughput, candidate.name, throughput))

output = root / "selection.json"
if not candidates:
    output.write_text(json.dumps({"selected": None, "reason": "no valid candidate"}, indent=2) + "\n")
    print(output)
    raise SystemExit(1)

candidates.sort()
worst_p99, _, name, throughput = candidates[0]
output.write_text(json.dumps({
    "selected": name,
    "worst_p99_ms": worst_p99,
    "median_requests_per_second": throughput,
}, indent=2, sort_keys=True) + "\n")
print(output)

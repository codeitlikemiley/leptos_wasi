#!/usr/bin/env python3
"""Keep-alive native load, alternating backend order, retaining oha JSON."""

import argparse
import json
import os
import statistics
import subprocess

from compare import ENDPOINTS, print_table, warmup


def measure(base, endpoint, duration, concurrency):
    command = ["oha", "--no-tui", "--output-format", "json", "--http-version", "1.1",
               "-w", "-z", f"{duration}s", "-c", str(concurrency),
               "-m", endpoint["method"]]
    if os.environ.get("LOADGEN_CPUS"):
        command = ["taskset", "-c", os.environ["LOADGEN_CPUS"], *command]
    for name, value in endpoint["headers"].items():
        command += ["-H", f"{name}: {value}"]
    if endpoint["body"] is not None:
        command += ["-d", endpoint["body"].decode()]
    command.append(base.rstrip("/") + endpoint["path"])
    # The tokio variable works on every oha release; the `--worker-threads`
    # flag only exists in newer ones.
    env = {**os.environ,
           "TOKIO_WORKER_THREADS": os.environ.get("LOADGEN_THREADS", "4")}
    result = json.loads(subprocess.check_output(command, text=True, env=env))
    validate(result)
    return result


def validate(result):
    statuses = result["statusCodeDistribution"]
    if not statuses or result["errorDistribution"] or any(
        not 200 <= int(status) < 300 for status in statuses
    ):
        raise RuntimeError(f"unsuccessful oha requests: {result}")


def median_row(name, results):
    return {
        "name": name,
        "requests_per_second": statistics.median(r["summary"]["requestsPerSec"] for r in results),
        "latency_ms": {
            percentile: statistics.median(r["latencyPercentiles"][percentile] * 1000 for r in results)
            for percentile in ("p50", "p99")
        },
        "failures": 0,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--wasi-url", required=True)
    parser.add_argument("--axum-url", required=True)
    parser.add_argument("--duration", type=float, default=30)
    parser.add_argument("--concurrency", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=25)
    parser.add_argument("--reuse", default="128")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    runs = int(os.environ.get("RUNS", "1"))
    concurrencies = [int(c) for c in os.environ.get("CONCURRENCY_LIST", str(args.concurrency)).split(",")]
    if runs < 1 or args.duration <= 0 or any(c < 1 for c in concurrencies):
        parser.error("runs, duration, and concurrency must be positive")
    records = []
    for run in range(runs):
        for ci, concurrency in enumerate(concurrencies):
            for ei, endpoint in enumerate(ENDPOINTS):
                order = ("wasi", "axum") if (run + ci + ei) % 2 == 0 else ("axum", "wasi")
                for backend in order:
                    base = getattr(args, f"{backend}_url")
                    warmup(base, endpoint, args.warmup, 10)
                    print(f"run={run + 1} c={concurrency} {endpoint['name']} {backend}", flush=True)
                    result = measure(base, endpoint, args.duration, concurrency)
                    record = dict(run=run + 1, concurrency=concurrency,
                                  endpoint=endpoint["name"], backend=backend, result=result)
                    records.append(record)
                    print(json.dumps(record), flush=True)
    for concurrency in concurrencies:
        rows = {}
        for backend in ("wasi", "axum"):
            rows[backend] = [median_row(endpoint["name"], [
                r["result"] for r in records if r["concurrency"] == concurrency
                and r["backend"] == backend and r["endpoint"] == endpoint["name"]
            ]) for endpoint in ENDPOINTS]
        print(f"\nOHA MEDIANS across {runs} runs; HTTP/1.1 keep-alive; alternating order")
        print_table(rows["wasi"], rows["axum"], args.duration, concurrency, args.warmup, args.reuse)
    if args.json:
        print(json.dumps({"records": records}))


if __name__ == "__main__":
    main()

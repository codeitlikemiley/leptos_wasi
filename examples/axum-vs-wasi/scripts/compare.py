#!/usr/bin/env python3
"""Multi-endpoint closed-loop probe for the axum vs WASI comparison.

Prints a side-by-side table. Does not invent numbers: it only reports what
this process just measured. Requires both servers to already be listening.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
import statistics
import sys
import threading
import time
import urllib.error
import urllib.request
from typing import Any


ENDPOINTS: list[dict[str, Any]] = [
    {
        "name": "GET /",
        "method": "GET",
        "path": "/",
        "headers": {},
        "body": None,
    },
    {
        "name": "GET /api/get_test",
        "method": "GET",
        "path": "/api/get_test",
        "headers": {},
        "body": None,
    },
    {
        "name": "POST /api/post_test",
        "method": "POST",
        "path": "/api/post_test",
        "headers": {"Content-Type": "application/json"},
        "body": b'{"msg":"hello"}',
    },
    {
        "name": "POST /api/form_test",
        "method": "POST",
        "path": "/api/form_test",
        "headers": {"Content-Type": "application/x-www-form-urlencoded"},
        "body": b"msg=hello",
    },
    {
        "name": "GET /static/hello.txt",
        "method": "GET",
        "path": "/static/hello.txt",
        "headers": {},
        "body": None,
    },
]


def percentile(values: list[float], quantile: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = max(0, math.ceil(quantile * len(ordered)) - 1)
    return ordered[index]


def request_once(
    url: str,
    timeout: float,
    method: str,
    headers: dict[str, str],
    body: bytes | None,
) -> tuple[float, int]:
    started = time.perf_counter()
    request = urllib.request.Request(
        url, data=body, headers=headers, method=method
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        error.read()
        status = error.code
    return (time.perf_counter() - started) * 1000.0, status


def warmup(
    base: str,
    endpoint: dict[str, Any],
    count: int,
    timeout: float,
) -> None:
    url = base.rstrip("/") + endpoint["path"]
    for _ in range(count):
        latency, status = request_once(
            url,
            timeout,
            endpoint["method"],
            endpoint["headers"],
            endpoint["body"],
        )
        del latency
        if not 200 <= status < 400:
            raise RuntimeError(f"{endpoint['method']} {url} returned {status}")


def probe(
    base: str,
    endpoint: dict[str, Any],
    duration: float,
    concurrency: int,
    timeout: float,
) -> dict[str, Any]:
    url = base.rstrip("/") + endpoint["path"]
    deadline = time.monotonic() + duration
    latencies: list[float] = []
    statuses: dict[int, int] = {}
    failures = 0
    lock = threading.Lock()

    def worker() -> None:
        nonlocal failures
        while time.monotonic() < deadline:
            try:
                latency, status = request_once(
                    url,
                    timeout,
                    endpoint["method"],
                    endpoint["headers"],
                    endpoint["body"],
                )
                with lock:
                    latencies.append(latency)
                    statuses[status] = statuses.get(status, 0) + 1
                    if not 200 <= status < 400:
                        failures += 1
            except (OSError, TimeoutError):
                with lock:
                    failures += 1

    started = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(
        max_workers=concurrency
    ) as executor:
        futures = [executor.submit(worker) for _ in range(concurrency)]
        for future in futures:
            future.result()
    elapsed = time.monotonic() - started
    return {
        "name": endpoint["name"],
        "url": url,
        "duration_seconds": elapsed,
        "concurrency": concurrency,
        "requests": len(latencies),
        "requests_per_second": len(latencies) / elapsed if elapsed else 0.0,
        "failures": failures,
        "statuses": statuses,
        "achieved_concurrency": (
            sum(latencies) / (elapsed * 1000.0) if elapsed else 0.0
        ),
        "latency_ms": {
            "mean": statistics.fmean(latencies) if latencies else 0.0,
            "p50": percentile(latencies, 0.50),
            "p95": percentile(latencies, 0.95),
            "p99": percentile(latencies, 0.99),
            "max": max(latencies, default=0.0),
        },
    }


def fmt(value: float, digits: int = 1) -> str:
    return f"{value:.{digits}f}"


def ratio(left: float, right: float) -> str:
    if left <= 0.0:
        return "n/a"
    return f"{right / left:.2f}x"


def print_table(
    wasi: list[dict[str, Any]],
    axum: list[dict[str, Any]],
    duration: float,
    concurrency: int,
    warmup_count: int,
    reuse: str,
) -> None:
    print(
        f"duration={duration}s concurrency={concurrency} "
        f"warmup={warmup_count} wasmtime_reuse={reuse}"
    )
    print(
        "Fairness: same machine, both warmed, same duration/concurrency. "
        "WASI is Preview 2 + RouteTable."
    )
    print()
    header = (
        f"{'endpoint':<24} {'backend':<8} {'rps':>10} {'p50 ms':>10} "
        f"{'p99 ms':>10} {'errors':>8}"
    )
    print(header)
    print("-" * len(header))
    for wasi_row, axum_row in zip(wasi, axum):
        for backend, row in (("wasi", wasi_row), ("axum", axum_row)):
            print(
                f"{row['name']:<24} {backend:<8} "
                f"{fmt(row['requests_per_second']):>10} "
                f"{fmt(row['latency_ms']['p50']):>10} "
                f"{fmt(row['latency_ms']['p99']):>10} "
                f"{row['failures']:>8}"
            )
        print()
    print("ratio (axum / wasi); >1x rps means axum handled more requests")
    print(f"{'endpoint':<24} {'rps':>10} {'p50':>10} {'p99':>10}")
    print("-" * 56)
    for wasi_row, axum_row in zip(wasi, axum):
        print(
            f"{wasi_row['name']:<24} "
            f"{ratio(wasi_row['requests_per_second'], axum_row['requests_per_second']):>10} "
            f"{ratio(wasi_row['latency_ms']['p50'], axum_row['latency_ms']['p50']):>10} "
            f"{ratio(wasi_row['latency_ms']['p99'], axum_row['latency_ms']['p99']):>10}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--wasi-url", default="http://127.0.0.1:3000")
    parser.add_argument("--axum-url", default="http://127.0.0.1:3001")
    parser.add_argument("--duration", type=float, default=10.0)
    parser.add_argument("--concurrency", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=25)
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument(
        "--reuse",
        default="128",
        help="Documented WASI instance-reuse count; printed, not applied here",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Also emit the raw measurements as JSON after the table",
    )
    args = parser.parse_args()

    wasi_results: list[dict[str, Any]] = []
    axum_results: list[dict[str, Any]] = []
    for endpoint in ENDPOINTS:
        try:
            warmup(args.wasi_url, endpoint, args.warmup, args.timeout)
            warmup(args.axum_url, endpoint, args.warmup, args.timeout)
        except Exception as error:  # noqa: BLE001 — surface warmup failures
            print(f"warmup failed: {error}", file=sys.stderr)
            return 1
        print(f"measuring {endpoint['name']} on WASI...", flush=True)
        wasi_results.append(
            probe(
                args.wasi_url,
                endpoint,
                args.duration,
                args.concurrency,
                args.timeout,
            )
        )
        print(f"measuring {endpoint['name']} on axum...", flush=True)
        axum_results.append(
            probe(
                args.axum_url,
                endpoint,
                args.duration,
                args.concurrency,
                args.timeout,
            )
        )

    print()
    print_table(
        wasi_results,
        axum_results,
        args.duration,
        args.concurrency,
        args.warmup,
        args.reuse,
    )
    if args.json:
        print()
        print(
            json.dumps(
                {"wasi": wasi_results, "axum": axum_results},
                indent=2,
                sort_keys=True,
            )
        )
    failures = sum(row["failures"] for row in wasi_results + axum_results)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())

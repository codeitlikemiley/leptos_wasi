#!/usr/bin/env python3
"""Concurrent HTTP soak/load probe for leptos_wasi runtime deployments."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
import statistics
import subprocess
import threading
import time
import urllib.error
import urllib.request


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
) -> tuple[float, float, int]:
    started = time.perf_counter()
    request = urllib.request.Request(
        url,
        data=body,
        headers=headers,
        method=method,
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            first = response.read(1)
            first_byte_ms = (time.perf_counter() - started) * 1000.0
            response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        first = error.read(1)
        first_byte_ms = (time.perf_counter() - started) * 1000.0
        error.read()
        status = error.code
    if not first:
        # Empty responses still have an observable header arrival time.
        first_byte_ms = (time.perf_counter() - started) * 1000.0
    return (
        (time.perf_counter() - started) * 1000.0,
        first_byte_ms,
        status,
    )


def final_quarter_start(samples: list[int]) -> int:
    """Index the final quarter starts at, never the last sample itself.

    At exactly four samples `3 * n // 4` is the last index, so the growth was
    a sample minus itself: always zero, and a gate that could not fail. Back
    the boundary off by at least one sample so the window always spans a real
    interval. Long runs are unaffected.
    """
    count = len(samples)
    return min(count - 2, count * 3 // 4)


def read_rss_kib(pid: int) -> int | None:
    try:
        output = subprocess.check_output(
            ["ps", "-o", "rss=", "-p", str(pid)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        return int(output) if output else None
    except (OSError, subprocess.CalledProcessError, ValueError):
        return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("url")
    parser.add_argument("--duration", type=float, default=600.0)
    parser.add_argument("--concurrency", type=int, default=100)
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--pid", type=int, default=None)
    parser.add_argument("--method", default="GET")
    parser.add_argument("--header", action="append", default=[])
    parser.add_argument("--body", default=None)
    args = parser.parse_args()

    headers: dict[str, str] = {}
    for header in args.header:
        name, separator, value = header.partition(":")
        if not separator or not name.strip():
            parser.error(f"invalid header: {header}")
        if name.strip().lower() in headers:
            parser.error(f"duplicate header: {name.strip()}")
        headers[name.strip().lower()] = value.strip()
    body = args.body.encode() if args.body is not None else None

    deadline = time.monotonic() + args.duration
    latencies: list[float] = []
    first_byte_latencies: list[float] = []
    statuses: dict[int, int] = {}
    failures = 0
    lock = threading.Lock()
    rss_samples: list[int] = []
    rss_timeline: list[dict[str, float | int]] = []

    def worker() -> None:
        nonlocal failures
        while time.monotonic() < deadline:
            try:
                latency, first_byte, status = request_once(
                    args.url,
                    args.timeout,
                    args.method,
                    headers,
                    body,
                )
                with lock:
                    latencies.append(latency)
                    first_byte_latencies.append(first_byte)
                    statuses[status] = statuses.get(status, 0) + 1
                    if not 200 <= status < 400:
                        failures += 1
            except (OSError, TimeoutError):
                with lock:
                    failures += 1

    started = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(
        max_workers=args.concurrency
    ) as executor:
        futures = [executor.submit(worker) for _ in range(args.concurrency)]
        while any(not future.done() for future in futures):
            if args.pid is not None:
                rss = read_rss_kib(args.pid)
                if rss is not None:
                    rss_samples.append(rss)
                    rss_timeline.append(
                        {
                            "elapsed_seconds": round(
                                time.monotonic() - started, 3
                            ),
                            "rss_kib": rss,
                        }
                    )
            time.sleep(min(1.0, max(0.05, deadline - time.monotonic())))
        for future in futures:
            future.result()

    elapsed = time.monotonic() - started
    result = {
        "url": args.url,
        "duration_seconds": elapsed,
        "concurrency": args.concurrency,
        "requests": len(latencies),
        "requests_per_second": len(latencies) / elapsed if elapsed else 0.0,
        "failures": failures,
        "statuses": statuses,
        "latency_ms": {
            "mean": statistics.fmean(latencies) if latencies else 0.0,
            "p50": percentile(latencies, 0.50),
            "p95": percentile(latencies, 0.95),
            "p99": percentile(latencies, 0.99),
            "max": max(latencies, default=0.0),
        },
        "first_byte_ms": {
            "mean": (
                statistics.fmean(first_byte_latencies)
                if first_byte_latencies
                else 0.0
            ),
            "p50": percentile(first_byte_latencies, 0.50),
            "p95": percentile(first_byte_latencies, 0.95),
            "p99": percentile(first_byte_latencies, 0.99),
            "max": max(first_byte_latencies, default=0.0),
        },
        "rss_kib": {
            "start": rss_samples[0] if rss_samples else None,
            "end": rss_samples[-1] if rss_samples else None,
            "max": max(rss_samples, default=None),
            "samples": len(rss_samples),
            "timeline": rss_timeline,
            "last_quarter_growth": (
                rss_samples[-1] - rss_samples[final_quarter_start(rss_samples)]
                if len(rss_samples) >= 4
                else None
            ),
        },
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())

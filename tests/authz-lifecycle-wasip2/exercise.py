#!/usr/bin/env python3
"""Exercise the real-host WASIp2 authorization and WaitPoll lifecycle."""

from __future__ import annotations

import argparse
import http.client
import time
import urllib.parse


class LifecycleFailure(RuntimeError):
    """A WASIp2 lifecycle assertion failed."""


def request(
    host: str, port: int, path: str, expected: int
) -> tuple[float, list[tuple[str, str]]]:
    """Issue one bounded request and assert its empty response contract."""
    connection = http.client.HTTPConnection(host, port, timeout=3.0)
    started = time.monotonic()
    connection.request(
        "GET",
        f"{path}?probe=wasip2-secret-query-sentinel",
        headers={"Cookie": "wasip2-secret-cookie-sentinel=1"},
    )
    response = connection.getresponse()
    body = response.read()
    elapsed = time.monotonic() - started
    headers = response.getheaders()
    connection.close()
    if response.status != expected:
        raise LifecycleFailure(
            f"{path} returned {response.status}, expected {expected}"
        )
    if body:
        raise LifecycleFailure(f"{path} returned an unexpected body")
    values = [
        value
        for name, value in headers
        if name.casefold() == "x-wasip2-pollable-depth"
    ]
    if values != ["0"]:
        raise LifecycleFailure(f"{path} retained pollables: {values!r}")
    return elapsed, headers


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    arguments = parser.parse_args()
    parsed = urllib.parse.urlsplit(arguments.base_url)
    if parsed.scheme != "http" or parsed.hostname is None or parsed.port is None:
        raise LifecycleFailure("base URL must be an explicit loopback HTTP URL")
    if parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise LifecycleFailure("WASIp2 lifecycle tests are loopback-only")

    request(parsed.hostname, parsed.port, "/allow", 204)
    for _ in range(10):
        elapsed, _headers = request(
            parsed.hostname, parsed.port, "/timeout", 504
        )
        if not 0.4 <= elapsed < 1.5:
            raise LifecycleFailure(
                f"absolute timeout completed outside its window: {elapsed:.3f}s"
            )
        request(parsed.hostname, parsed.port, "/queue-depth", 204)
    request(parsed.hostname, parsed.port, "/recovery", 204)
    request(parsed.hostname, parsed.port, "/queue-depth", 204)
    print("WASIp2 authorization and WaitPoll lifecycle passed")


if __name__ == "__main__":
    main()

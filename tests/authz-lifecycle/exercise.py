#!/usr/bin/env python3
"""Exercise the composed authentication and authorization lifecycle chain."""

from __future__ import annotations

import argparse
import concurrent.futures
import http.client
import json
import socket
import time
import urllib.parse


AUTHORIZATION = "Bearer allow"
COOKIE = "lifecycle_cookie=middleware-secret-cookie-sentinel"
SPOOFED_IDENTITY = "middleware-secret-identity-sentinel"
QUERY_SENTINEL = "middleware-secret-query-sentinel"
FIRST_FRAME = b"first-frame\n"


class LifecycleFailure(RuntimeError):
    """A lifecycle contract assertion failed."""


class Client:
    """Small bounded HTTP/1.1 client for lifecycle assertions."""

    def __init__(self, base_url: str) -> None:
        parsed = urllib.parse.urlsplit(base_url)
        if parsed.scheme != "http" or parsed.hostname is None or parsed.port is None:
            raise LifecycleFailure("base URL must be an explicit loopback HTTP URL")
        if parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
            raise LifecycleFailure("lifecycle tests are loopback-only")
        self.host = parsed.hostname
        self.port = parsed.port

    def connection(self, timeout: float = 5.0) -> http.client.HTTPConnection:
        """Create one bounded connection."""
        return http.client.HTTPConnection(self.host, self.port, timeout=timeout)

    def headers(
        self, request_id: str, *, authenticated: bool = True
    ) -> dict[str, str]:
        """Return the hostile client headers shared by every request."""
        headers = {
            "Cookie": COOKIE,
            "x-request-id": request_id,
            "x-wasi-auth-context": SPOOFED_IDENTITY,
        }
        if authenticated:
            headers["Authorization"] = AUTHORIZATION
        return headers

    def request(
        self,
        path: str,
        request_id: str,
        *,
        authenticated: bool = True,
        timeout: float = 5.0,
    ) -> tuple[int, list[tuple[str, str]], bytes, float]:
        """Issue one request and fully collect its bounded response."""
        connection = self.connection(timeout)
        started = time.monotonic()
        connection.request(
            "GET",
            path,
            headers=self.headers(request_id, authenticated=authenticated),
        )
        response = connection.getresponse()
        try:
            body = response.read()
        finally:
            connection.close()
        return response.status, response.getheaders(), body, time.monotonic() - started


def header_values(headers: list[tuple[str, str]], name: str) -> list[str]:
    """Return case-insensitive response-header values."""
    expected = name.casefold()
    return [value for candidate, value in headers if candidate.casefold() == expected]


def assert_boundary_headers(
    headers: list[tuple[str, str]], *, downstream: bool
) -> None:
    """Assert browser responses contain no trusted request credentials."""
    names = [name.casefold() for name, _ in headers]
    if "authorization" in names or any(name.startswith("x-wasi-auth-") for name in names):
        raise LifecycleFailure("trusted authentication metadata escaped in a response")
    marker = header_values(headers, "x-authz-lifecycle-downstream")
    expected = ["invoked-once"] if downstream else []
    if marker != expected:
        raise LifecycleFailure(
            f"downstream marker mismatch: expected {expected!r}, found {marker!r}"
        )


def assert_status(
    client: Client,
    path: str,
    request_id: str,
    expected: int,
    *,
    downstream: bool,
    maximum_seconds: float = 5.0,
    authenticated: bool = True,
) -> bytes:
    """Assert one fully collected HTTP outcome."""
    status, headers, body, elapsed = client.request(
        path, request_id, authenticated=authenticated
    )
    if status != expected:
        raise LifecycleFailure(
            f"{request_id} returned {status}, expected {expected}; body={body[:80]!r}"
        )
    if elapsed >= maximum_seconds:
        raise LifecycleFailure(
            f"{request_id} exceeded {maximum_seconds:.3f}s ({elapsed:.3f}s)"
        )
    assert_boundary_headers(headers, downstream=downstream)
    if expected == 503:
        assert_unavailable_contract(headers, body)
    return body


def assert_unavailable_contract(
    headers: list[tuple[str, str]], body: bytes
) -> None:
    """Assert a provider failure is generic, non-cacheable, and retryable."""
    if body:
        raise LifecycleFailure("authorization exposed a provider failure body")
    if header_values(headers, "cache-control") != ["no-store"]:
        raise LifecycleFailure("authorization 503 omitted cache-control: no-store")
    if header_values(headers, "retry-after") != ["1"]:
        raise LifecycleFailure("authorization 503 omitted its bounded retry hint")


def await_recovery(
    client: Client,
    path: str,
    request_id_prefix: str,
    *,
    maximum_seconds: float = 5.0,
) -> None:
    """Require the composed host and PEP to recover within one bounded window."""
    deadline = time.monotonic() + maximum_seconds
    attempt = 0
    while time.monotonic() < deadline:
        status, headers, body, _elapsed = client.request(
            path, f"{request_id_prefix}-{attempt}"
        )
        if status == 200:
            assert_boundary_headers(headers, downstream=True)
            return
        if status != 503:
            raise LifecycleFailure(
                f"recovery returned {status}, expected temporary 503 or final 200"
            )
        assert_boundary_headers(headers, downstream=False)
        assert_unavailable_contract(headers, body)
        attempt += 1
        time.sleep(0.1)
    raise LifecycleFailure(
        f"authorization did not recover within {maximum_seconds:.1f}s"
    )


def exercise_delayed_stream(client: Client) -> None:
    """Prove authorization does not buffer a delayed Leptos response."""
    connection = client.connection()
    path = f"/static/delayed.stream?probe={QUERY_SENTINEL}"
    started = time.monotonic()
    connection.request("GET", path, headers=client.headers("lifecycle-allow-delayed"))
    response = connection.getresponse()
    if response.status != 200:
        raise LifecycleFailure(f"delayed stream returned {response.status}")
    assert_boundary_headers(response.getheaders(), downstream=True)
    first = response.read(len(FIRST_FRAME))
    first_byte_elapsed = time.monotonic() - started
    if first != FIRST_FRAME:
        raise LifecycleFailure(f"unexpected delayed first frame: {first!r}")
    if first_byte_elapsed >= 0.4:
        raise LifecycleFailure(
            f"authorization buffered the delayed first frame ({first_byte_elapsed:.3f}s)"
        )
    remainder = response.read()
    total_elapsed = time.monotonic() - started
    connection.close()
    if remainder != b"second-frame\n":
        raise LifecycleFailure(f"unexpected delayed remainder: {remainder!r}")
    if total_elapsed < 0.4:
        raise LifecycleFailure("delayed response did not exercise a real async gap")


def exercise_failing_stream(client: Client) -> None:
    """Prove a committed producer failure does not poison the next request."""
    connection = client.connection()
    path = f"/static/failing.stream?probe={QUERY_SENTINEL}"
    connection.request("GET", path, headers=client.headers("lifecycle-allow-failing"))
    response = connection.getresponse()
    if response.status != 200:
        raise LifecycleFailure(f"failing stream returned {response.status}")
    assert_boundary_headers(response.getheaders(), downstream=True)
    first = response.read(len(FIRST_FRAME))
    if first != FIRST_FRAME:
        raise LifecycleFailure(f"failing stream lost its committed frame: {first!r}")
    try:
        remainder = response.read()
    except (http.client.IncompleteRead, ConnectionError, OSError):
        remainder = b""
    finally:
        connection.close()
    if remainder:
        raise LifecycleFailure(f"failing stream emitted data after its error: {remainder!r}")


def exercise_trailer(client: Client) -> None:
    """Prove opaque downstream trailers survive the authn/authz components."""
    request = (
        "GET /__authz_lifecycle/trailers HTTP/1.1\r\n"
        f"Host: {client.host}:{client.port}\r\n"
        f"Authorization: {AUTHORIZATION}\r\n"
        f"Cookie: {COOKIE}\r\n"
        "x-request-id: lifecycle-allow-trailers\r\n"
        f"x-wasi-auth-context: {SPOOFED_IDENTITY}\r\n"
        "TE: trailers\r\n"
        "Connection: close\r\n\r\n"
    ).encode("ascii")
    with socket.create_connection((client.host, client.port), timeout=5.0) as stream:
        stream.sendall(request)
        chunks: list[bytes] = []
        while True:
            chunk = stream.recv(16 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
    raw = b"".join(chunks)
    if not raw.startswith(b"HTTP/1.1 200"):
        raise LifecycleFailure(f"trailer response was not 200: {raw[:120]!r}")
    if b"trailer-body\n" not in raw:
        raise LifecycleFailure("trailer response body was lost")
    trailer = b"\r\nx-authz-lifecycle-check: preserved\r\n"
    if trailer not in raw.lower():
        raise LifecycleFailure(f"downstream trailer was not preserved: {raw[-240:]!r}")
    lower = raw.lower()
    if b"\r\nx-wasi-auth-" in lower or b"\r\nauthorization:" in lower:
        raise LifecycleFailure("trusted authentication metadata escaped in trailer response")


def assert_pdp_idle(pdp_url: str) -> None:
    """Assert canceled transport work returned every fixture guard to zero."""
    parsed = urllib.parse.urlsplit(pdp_url)
    connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=2.0)
    connection.request("GET", "/__authz_lifecycle/status")
    response = connection.getresponse()
    body = response.read()
    connection.close()
    if response.status != 200:
        raise LifecycleFailure(f"fault PDP status returned {response.status}")
    status = json.loads(body)
    expected = {
        "active_requests": 0,
        "active_bodies": 0,
        "saturation_in_flight": 0,
    }
    if status != expected:
        raise LifecycleFailure(f"fault PDP retained lifecycle work: {status!r}")


def run_allow(client: Client, pdp_url: str) -> None:
    exercise_delayed_stream(client)
    exercise_failing_stream(client)
    exercise_trailer(client)
    assert_status(
        client,
        "/api/get_test",
        "lifecycle-allow-recovery",
        200,
        downstream=True,
    )
    assert_pdp_idle(pdp_url)


def run_faults(client: Client, pdp_url: str) -> None:
    for request_id in ("lifecycle-provider-503", "lifecycle-malformed"):
        assert_status(client, "/api/get_test", request_id, 503, downstream=False)
    for request_id in ("lifecycle-timeout-first-byte", "lifecycle-timeout-body"):
        assert_status(
            client,
            "/api/get_test",
            request_id,
            503,
            downstream=False,
            maximum_seconds=1.0,
        )
    time.sleep(1.25)
    assert_pdp_idle(pdp_url)
    assert_status(
        client,
        "/api/get_test",
        "lifecycle-fault-recovery",
        200,
        downstream=True,
    )


def run_saturation(client: Client, pdp_url: str) -> None:
    def request(index: int) -> None:
        assert_status(
            client,
            "/api/get_test",
            f"lifecycle-saturation-case-{index}",
            503,
            downstream=False,
            maximum_seconds=2.0,
            authenticated=False,
        )

    with concurrent.futures.ThreadPoolExecutor(max_workers=32) as executor:
        futures = [executor.submit(request, index) for index in range(32)]
        for future in futures:
            future.result(timeout=5.0)
    time.sleep(0.5)
    assert_pdp_idle(pdp_url)
    await_recovery(
        client, "/api/get_test", "lifecycle-recovery-after-saturation"
    )


def run_disconnect(client: Client, pdp_url: str) -> None:
    request = (
        "GET /api/get_test HTTP/1.1\r\n"
        f"Host: {client.host}:{client.port}\r\n"
        f"Authorization: {AUTHORIZATION}\r\n"
        f"Cookie: {COOKIE}\r\n"
        "x-request-id: lifecycle-disconnect\r\n"
        f"x-wasi-auth-context: {SPOOFED_IDENTITY}\r\n"
        "Connection: close\r\n\r\n"
    ).encode("ascii")
    stream = socket.create_connection((client.host, client.port), timeout=2.0)
    stream.sendall(request)
    time.sleep(0.05)
    stream.close()
    time.sleep(1.25)
    assert_pdp_idle(pdp_url)
    assert_status(
        client,
        "/api/get_test",
        "lifecycle-disconnect-recovery",
        200,
        downstream=True,
    )


def run_outage(client: Client) -> None:
    assert_status(
        client,
        "/api/get_test",
        "lifecycle-provider-outage",
        503,
        downstream=False,
        maximum_seconds=1.0,
    )


def run_recovery(client: Client, pdp_url: str) -> None:
    await_recovery(client, "/api/get_test", "lifecycle-outage-recovery")
    assert_pdp_idle(pdp_url)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=("allow", "faults", "saturation", "disconnect", "outage", "recovery"))
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--pdp-url")
    arguments = parser.parse_args()
    client = Client(arguments.base_url)
    if arguments.phase == "outage":
        run_outage(client)
    else:
        if arguments.pdp_url is None:
            raise LifecycleFailure("--pdp-url is required for this phase")
        {
            "allow": run_allow,
            "faults": run_faults,
            "saturation": run_saturation,
            "disconnect": run_disconnect,
            "recovery": run_recovery,
        }[arguments.phase](client, arguments.pdp_url)
    print(f"authz lifecycle phase passed: {arguments.phase}")


if __name__ == "__main__":
    main()

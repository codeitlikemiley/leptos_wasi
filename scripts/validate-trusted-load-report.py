#!/usr/bin/env python3
"""Validate trusted-load report accounting and required evidence fields."""

import json
import pathlib
import sys


def validate(report: dict) -> None:
    if report.get("schema") != 2:
        raise AssertionError("trusted-load report schema must be 2")
    totals = report["totals"]
    terminal = totals["completed"] + totals["transport_failures"] + totals["hung"]
    if totals["attempted"] != terminal:
        raise AssertionError("every attempt must have exactly one terminal outcome")
    if totals["status_failures"] > totals["completed"]:
        raise AssertionError("status failures must be completed responses")
    for field in ("response_headers_ms", "first_body_byte_ms", "latency_ms"):
        if field not in report:
            raise AssertionError(f"trusted-load report is missing {field}")
    for samples in report.get("processes", {}).values():
        if not samples:
            raise AssertionError("declared process has no resource samples")


def main() -> None:
    if len(sys.argv) == 1:
        raise SystemExit("usage: validate-trusted-load-report.py REPORT...")
    for value in sys.argv[1:]:
        report = json.loads(pathlib.Path(value).read_text(encoding="utf-8"))
        validate(report)


if __name__ == "__main__":
    main()

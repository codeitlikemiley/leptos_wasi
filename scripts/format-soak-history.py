#!/usr/bin/env python3
"""Print markdown table rows from CI ten-minute soak comparison JSON.

CI uploads one artifact per lane containing `soak-<host>-<preview>-comparison.json`
and a sibling `*-absolute.json`. Artifacts expire after 14 days; paste the
printed rows into SOAK_HISTORY.md after a merge to main.

Reuse is not in the JSON. Pass `--reuse host/preview=value` from the soak job
log (`instance reuse count: 128` or `host default`). Spin has no Wasmtime
reuse flag and defaults to `n/a`.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


LANE_LABELS = {
    ("wasmtime", "p2"): "Wasmtime P2",
    ("wasmtime", "p3"): "Wasmtime P3",
    ("spin", "p2"): "Spin P2",
    ("spin", "p3"): "Spin P3",
}
LANE_ORDER = list(LANE_LABELS)
DEFAULT_BASELINE_REFS = {
    ("wasmtime", "p2"): "9689c68",
    ("wasmtime", "p3"): "663e1a9",
    ("spin", "p2"): "9689c68",
}
COMPARISON_NAME = re.compile(r"^soak-(wasmtime|spin)-(p2|p3)-comparison\.json$")
HEADER = (
    "| Date | Commit | Lane | Reuse | Baseline | "
    "rps (base to cand) | p99 ms (base to cand) | Δ rps | Δ p99 | Gate | Notes |"
)
SEPARATOR = "|---|---|---|---|---|---:|---:|---:|---:|---|---|"
MISSING = "—"


def read_json(path: Path) -> dict[str, object]:
    with path.open(encoding="utf-8") as source:
        payload = json.load(source)
    if not isinstance(payload, dict):
        raise ValueError(f"{path} is not a JSON object")
    return payload


def nested_mapping(result: dict[str, object], field: str) -> dict[str, object]:
    value = result.get(field)
    if not isinstance(value, dict):
        raise ValueError(f"{field!r} is missing from the comparison report")
    return value


def nested_number(values: dict[str, object], field: str, label: str) -> float:
    value = values.get(field)
    if not isinstance(value, (int, float)):
        raise ValueError(f"{label} is not numeric")
    return float(value)


def parse_lane_name(name: str) -> tuple[str, str]:
    matched = COMPARISON_NAME.fullmatch(name)
    if matched is None:
        raise ValueError(
            f"{name} is not a soak comparison file "
            "(expected soak-<wasmtime|spin>-<p2|p3>-comparison.json)"
        )
    return matched.group(1), matched.group(2)


def parse_lane_value(raw: str, flag: str) -> tuple[tuple[str, str], str]:
    if "=" not in raw:
        raise ValueError(f"{flag} expected host/preview=value, got {raw!r}")
    lane, value = raw.split("=", 1)
    if "/" not in lane:
        raise ValueError(f"{flag} expected host/preview=value, got {raw!r}")
    host, preview = lane.split("/", 1)
    key = (host.lower(), preview.lower())
    if key not in LANE_LABELS:
        raise ValueError(f"unknown lane {lane!r}")
    if not value:
        raise ValueError(f"{flag} value for {lane} is empty")
    if "|" in value:
        raise ValueError(f"{flag} value must not contain '|'")
    return key, value


def collect_comparison_paths(inputs: list[Path]) -> list[Path]:
    found: list[Path] = []
    seen: set[Path] = set()
    for path in inputs:
        if path.is_dir():
            matches = sorted(
                path.rglob("soak-*-comparison.json"),
                key=lambda item: (
                    LANE_ORDER.index(parse_lane_name(item.name))
                    if COMPARISON_NAME.fullmatch(item.name)
                    else 99,
                    str(item),
                ),
            )
            if not matches:
                raise ValueError(f"no soak-*-comparison.json under {path}")
            candidates = matches
        elif path.is_file():
            candidates = [path]
        else:
            raise ValueError(f"{path} does not exist")
        for candidate in candidates:
            resolved = candidate.resolve()
            if resolved in seen:
                continue
            parse_lane_name(candidate.name)
            seen.add(resolved)
            found.append(candidate)
    return found


def signed_percent(value: float) -> str:
    return f"{value:+.2f}%"


def cell(value: str | None) -> str:
    if value is None or value == "":
        return MISSING
    if "|" in value:
        raise ValueError("table cells must not contain '|'")
    return value


def commit_cell(sha: str | None, pr: str | None) -> str:
    short = sha[:7] if sha else None
    if short and pr:
        return f"{short} (#{pr.lstrip('#')})"
    if short:
        return short
    if pr:
        return f"#{pr.lstrip('#')}"
    return MISSING


def notes_cell(run: str | None, notes: str | None) -> str:
    parts: list[str] = []
    if run:
        parts.append(f"CI run {run}")
    if notes:
        parts.append(notes)
    return cell("; ".join(parts) if parts else None)


def reuse_cell(
    lane: tuple[str, str],
    overrides: dict[tuple[str, str], str],
) -> str:
    if lane in overrides:
        return cell(overrides[lane])
    if lane[0] == "spin":
        return "n/a"
    return MISSING


def baseline_cell(
    lane: tuple[str, str],
    overrides: dict[tuple[str, str], str],
) -> str:
    if lane in overrides:
        return cell(overrides[lane])
    return cell(DEFAULT_BASELINE_REFS.get(lane))


def gate_label(comparison: dict[str, object], absolute_path: Path) -> str:
    passed = comparison.get("passed")
    if not isinstance(passed, bool):
        raise ValueError("comparison 'passed' is not a boolean")
    if absolute_path.is_file():
        absolute = read_json(absolute_path)
        absolute_passed = absolute.get("passed")
        if not isinstance(absolute_passed, bool):
            raise ValueError(f"{absolute_path} 'passed' is not a boolean")
        passed = passed and absolute_passed
    return "pass" if passed else "fail"


def format_row(
    comparison_path: Path,
    *,
    date: str | None,
    sha: str | None,
    pr: str | None,
    run: str | None,
    notes: str | None,
    reuse: dict[tuple[str, str], str],
    baseline_refs: dict[tuple[str, str], str],
) -> str:
    lane = parse_lane_name(comparison_path.name)
    comparison = read_json(comparison_path)
    latency = nested_mapping(nested_mapping(comparison, "comparisons"), "latency_ms")
    throughput = nested_mapping(comparison, "throughput")
    baseline_rps = nested_number(
        throughput, "baseline_requests_per_second", "throughput.baseline_requests_per_second"
    )
    candidate_rps = nested_number(
        throughput,
        "candidate_requests_per_second",
        "throughput.candidate_requests_per_second",
    )
    rps_delta = nested_number(throughput, "change_percent", "throughput.change_percent")
    baseline_p99 = nested_number(latency, "baseline_p99", "latency_ms.baseline_p99")
    candidate_p99 = nested_number(latency, "candidate_p99", "latency_ms.candidate_p99")
    p99_delta = nested_number(latency, "change_percent", "latency_ms.change_percent")
    absolute_path = comparison_path.with_name(
        comparison_path.name.replace("-comparison.json", "-absolute.json")
    )
    return " | ".join(
        [
            "",
            cell(date),
            commit_cell(sha, pr),
            LANE_LABELS[lane],
            reuse_cell(lane, reuse),
            baseline_cell(lane, baseline_refs),
            f"{baseline_rps:.2f} to {candidate_rps:.2f}",
            f"{baseline_p99:.2f} to {candidate_p99:.2f}",
            signed_percent(rps_delta),
            signed_percent(p99_delta),
            gate_label(comparison, absolute_path),
            notes_cell(run, notes),
            "",
        ]
    ).strip()


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Print SOAK_HISTORY.md rows from soak comparison JSON."
    )
    parser.add_argument(
        "paths",
        nargs="+",
        type=Path,
        help="soak-*-comparison.json files or directories that contain them",
    )
    parser.add_argument("--date", help="UTC date for the row, YYYY-MM-DD")
    parser.add_argument("--sha", help="merge or PR-head commit SHA")
    parser.add_argument("--pr", help="pull request number")
    parser.add_argument("--run", help="GitHub Actions run id")
    parser.add_argument("--notes", help="extra note appended after the run id")
    parser.add_argument(
        "--reuse",
        action="append",
        default=[],
        metavar="HOST/PREVIEW=VALUE",
        help="instance reuse, e.g. wasmtime/p2=128 or 'wasmtime/p3=host default'",
    )
    parser.add_argument(
        "--baseline-ref",
        action="append",
        default=[],
        metavar="HOST/PREVIEW=SHA",
        help="override the CI matrix baseline ref (repeatable)",
    )
    parser.add_argument(
        "--header",
        action="store_true",
        help="print the markdown table header before the rows",
    )
    return parser.parse_args(argv)


def keyed_values(raw_values: list[str], flag: str) -> dict[tuple[str, str], str]:
    parsed: dict[tuple[str, str], str] = {}
    for raw in raw_values:
        key, value = parse_lane_value(raw, flag)
        parsed[key] = value
    return parsed


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        reuse = keyed_values(args.reuse, "--reuse")
        baseline_refs = keyed_values(args.baseline_ref, "--baseline-ref")
        paths = collect_comparison_paths(args.paths)
        if args.header:
            print(HEADER)
            print(SEPARATOR)
        for path in paths:
            print(
                format_row(
                    path,
                    date=args.date,
                    sha=args.sha,
                    pr=args.pr,
                    run=args.run,
                    notes=args.notes,
                    reuse=reuse,
                    baseline_refs=baseline_refs,
                )
            )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

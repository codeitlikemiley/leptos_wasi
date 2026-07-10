#!/usr/bin/env python3
"""Print the exact ordered middleware names for one deployment profile."""

from __future__ import annotations

import argparse
import pathlib
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("profile")
    args = parser.parse_args()
    with (ROOT / "tests/middleware/deployment-policy.toml").open("rb") as source:
        profiles = tomllib.load(source)["profile"]
    matches = [profile for profile in profiles if profile["name"] == args.profile]
    if len(matches) != 1:
        raise SystemExit(f"unknown or duplicate deployment profile: {args.profile}")
    for component in matches[0]["middleware"]:
        print(component)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

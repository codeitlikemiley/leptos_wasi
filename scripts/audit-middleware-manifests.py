#!/usr/bin/env python3
"""Audit the pinned experimental middleware fixtures and public asset route."""

from __future__ import annotations

import argparse
import subprocess
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def load(relative: str) -> dict:
    with (ROOT / relative).open("rb") as source:
        return tomllib.load(source)


def middleware_dependencies(trigger: dict) -> list[dict]:
    dependencies = trigger.get("dependencies", {})
    middleware = dependencies.get("middleware", [])
    if not isinstance(middleware, list):
        raise AssertionError("dependencies.middleware must be an array")
    return middleware


def trigger_for(manifest: dict, route: str) -> dict:
    triggers = manifest.get("trigger", {}).get("http", [])
    matches = [trigger for trigger in triggers if trigger.get("route") == route]
    if len(matches) != 1:
        raise AssertionError(f"expected one HTTP trigger for {route}, found {len(matches)}")
    return matches[0]


def command_version(command: str) -> str:
    try:
        result = subprocess.run(
            [command, "--version"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError) as error:
        raise AssertionError(f"failed to run {command} --version") from error
    return f"{result.stdout}\n{result.stderr}".strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--composition-tools", action="store_true")
    parser.add_argument("--spin", action="store_true")
    parser.add_argument("--wasmtime", action="store_true")
    args = parser.parse_args()

    lock = load("tests/middleware/components.lock.toml")
    fixture = load("tests/middleware-fixture/Cargo.toml")
    test_manifest = load("tests/spin-p3-middleware-vnext.toml")
    counter_manifest = load("examples/counter/spin.middleware-vnext.toml")
    stable_counter = load("examples/counter/spin.toml")
    fixture_lock = (ROOT / "tests/middleware-fixture/Cargo.lock").read_text()
    workflow = (ROOT / ".github/workflows/main.yaml").read_text()

    sdk = fixture["dependencies"]["spin-sdk"]
    assert sdk["git"] == lock["spin_sdk_repository"]
    assert sdk["rev"] == lock["spin_sdk_revision"]
    assert sdk["default-features"] is False
    assert "http-middleware" in sdk["features"]
    assert lock["spin_sdk_revision"] in fixture_lock
    assert lock["spin_runtime_revision"] in workflow
    if lock["spin_runtime_default_features"] is False:
        assert "--no-default-features" in workflow
    assert f"wac-cli --version {lock['wac_cli_version']}" in workflow
    assert f"wasm-tools --version {lock['wasm_tools_version']}" in workflow

    test_stack = middleware_dependencies(trigger_for(test_manifest, "/..."))
    assert test_stack == [{"component": "middleware-fixture"}]

    counter_stack = middleware_dependencies(trigger_for(counter_manifest, "/..."))
    assert counter_stack == [{"component": "middleware-fixture"}]

    asset_trigger = trigger_for(counter_manifest, "/pkg/...")
    assert middleware_dependencies(asset_trigger) == [], (
        "the split-WASM asset trigger must remain public in the experimental example"
    )
    file_server_source = counter_manifest["component"]["counter-pkg"]["source"]
    stable_file_server_source = stable_counter["component"]["counter-pkg"]["source"]
    assert file_server_source == stable_file_server_source, (
        "the experimental split-WASM file server must use the stable manifest's "
        "verified source and digest"
    )

    for trigger in stable_counter.get("trigger", {}).get("http", []):
        assert middleware_dependencies(trigger) == [], (
            "the stable Spin manifest must not use vNext middleware syntax"
        )

    local_fixture = test_manifest["component"]["middleware-fixture"]
    assert set(local_fixture) == {"source"}, (
        "the protocol fixture must not receive filesystem, network, or state capabilities"
    )

    if args.composition_tools:
        assert lock["wac_cli_version"] in command_version("wac")
        assert lock["wasm_tools_version"] in command_version("wasm-tools")
    if args.spin:
        short_revision = lock["spin_runtime_revision"][:7]
        assert short_revision in command_version("spin"), (
            "Spin does not match the pinned experimental middleware runtime"
        )
    if args.wasmtime:
        assert lock["wasmtime_version"] in command_version("wasmtime")

    print(
        "middleware manifests are pinned; the stable manifest is unchanged; "
        "/pkg split assets remain public"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, KeyError, OSError, tomllib.TOMLDecodeError) as error:
        print(f"middleware manifest audit failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error

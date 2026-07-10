#!/usr/bin/env python3
"""Audit final-WASI middleware manifests, capabilities, and pinned tools."""

from __future__ import annotations

import argparse
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]
EXPECTED_STACK = ["request-id", "security-headers", "cors", "authn-policy"]
EXPECTED_DEPENDENCIES = [
    {"component": "request-id"},
    {"component": "security-headers"},
    {"component": "cors", "inherit_configuration": ["environment"]},
    {
        "component": "authn-policy",
        "inherit_configuration": ["environment", "allowed_outbound_hosts"],
    },
]
AUTHN_ENVIRONMENT = {
    "WASI_MIDDLEWARE_AUTHN_BROKER_URL",
    "WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS",
    "WASI_MIDDLEWARE_AUTHN_MODE",
    "WASI_MIDDLEWARE_SERVICE_ID",
    "WASI_MIDDLEWARE_AUTHN_AUDIENCES",
    "WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT",
    "WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK",
}


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
        raise AssertionError(
            f"expected one HTTP trigger for {route}, found {len(matches)}"
        )
    return matches[0]


def command_version(command: pathlib.Path | str) -> str:
    try:
        result = subprocess.run(
            [str(command), "--version"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError) as error:
        raise AssertionError(f"failed to run {command} --version") from error
    return f"{result.stdout}\n{result.stderr}".strip()


def resolve_tool(environment: str, name: str, version: str) -> pathlib.Path | str:
    configured = os.environ.get(environment)
    if configured:
        selected: pathlib.Path | str = pathlib.Path(configured)
    else:
        cache_root = pathlib.Path(
            os.environ.get(
                "LEPTOS_WASI_TOOL_ROOT",
                pathlib.Path.home() / ".cache" / "leptos-wasi-tools",
            )
        )
        cached = cache_root / f"{name}-{version}" / name
        selected = cached if cached.is_file() else name
    actual = command_version(selected)
    if version not in actual:
        raise AssertionError(
            f"{name} version mismatch; expected {version}, found: {actual}"
        )
    return selected


def audit_authn_configuration(primary: dict, route: str) -> None:
    environment = primary.get("environment", {})
    missing = AUTHN_ENVIRONMENT.difference(environment)
    if missing:
        raise AssertionError(f"route {route} is missing authn environment: {missing}")
    if environment["WASI_MIDDLEWARE_AUTHN_MODE"] != "optional":
        raise AssertionError("mixed public SSR/server-function apps require optional authn")
    if environment["WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK"] != "true":
        raise AssertionError("the local broker fixture requires explicit loopback opt-in")
    audiences = {
        value.strip()
        for value in environment["WASI_MIDDLEWARE_AUTHN_AUDIENCES"].split(",")
        if value.strip()
    }
    if environment["WASI_MIDDLEWARE_SERVICE_ID"] not in audiences:
        raise AssertionError("authn audiences must contain the exact service ID")

    broker_url = environment["WASI_MIDDLEWARE_AUTHN_BROKER_URL"]
    broker_origin = broker_url.split("/authenticate", maxsplit=1)[0]
    if primary.get("allowed_outbound_hosts") != [broker_origin]:
        raise AssertionError(
            f"route {route} must inherit only the broker origin {broker_origin}"
        )


def audit_native_manifest(manifest: dict, route: str) -> None:
    trigger = trigger_for(manifest, route)
    actual = middleware_dependencies(trigger)
    if actual != EXPECTED_DEPENDENCIES:
        raise AssertionError(
            f"route {route} middleware must be {EXPECTED_STACK}, found {actual}"
        )

    component_name = trigger["component"]
    components = manifest["component"]
    primary = components[component_name]
    audit_authn_configuration(primary, route)

    for component in EXPECTED_STACK:
        definition = components.get(component)
        if not isinstance(definition, dict) or set(definition) != {"source"}:
            raise AssertionError(
                f"middleware {component} must declare only its component source"
            )


def audit_composed_manifest(manifest: dict, route: str) -> None:
    trigger = trigger_for(manifest, route)
    if middleware_dependencies(trigger):
        raise AssertionError("precomposed final-WASI triggers must not use native middleware")
    primary = manifest["component"][trigger["component"]]
    if not str(primary.get("source", "")).endswith("-middleware.wasm"):
        raise AssertionError("behavioral middleware trigger must serve a composed artifact")
    audit_authn_configuration(primary, route)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--composition-tools", action="store_true")
    parser.add_argument("--spin", action="store_true")
    parser.add_argument("--spin-native-canary", action="store_true")
    parser.add_argument("--wasmtime", action="store_true")
    args = parser.parse_args()

    lock = load("tests/middleware/components.lock.toml")
    test_manifest = load("tests/spin-p3-middleware-vnext.toml")
    counter_manifest = load("examples/counter/spin.middleware-vnext.toml")
    composed_test_manifest = load("tests/spin-p3-middleware-composed.toml")
    composed_counter_manifest = load(
        "examples/counter/spin.middleware-composed.toml"
    )
    stable_counter = load("examples/counter/spin.toml")
    workflow = (ROOT / ".github/workflows/main.yaml").read_text()
    root_manifest = load("Cargo.toml")

    if lock["wasi_http"] != "0.3.0" or lock["wasip3_version"] != "0.7.0":
        raise AssertionError("middleware integration must use final WASI HTTP 0.3")
    if lock["schema"] != 2:
        raise AssertionError("middleware compatibility lock schema must be 2")
    middleware_lock = lock["middleware"]
    if middleware_lock["version"] != "0.2.0-alpha.1":
        raise AssertionError("middleware integration must target the breaking 0.2 alpha")
    for key in ("baseline_revision", "source_revision"):
        if re.fullmatch(r"[0-9a-f]{40}", middleware_lock[key]) is None:
            raise AssertionError(f"middleware {key} must be a full Git revision")
    if middleware_lock["baseline_revision"] == middleware_lock["source_revision"]:
        raise AssertionError("middleware source revision must advance beyond its baseline")
    if root_manifest["dependencies"]["wasip3"]["version"] != "=0.7.0":
        raise AssertionError("the library must pin wasip3 exactly")
    if lock["spin_middleware_final_wasi_supported"] is not False:
        raise AssertionError("native Spin middleware must remain an incompatibility canary")
    if lock["spin_final_wasi_supported"] is not False:
        raise AssertionError("stable Spin must remain a final-WASI incompatibility canary")
    if lock["spin_final_wasi_failure"] != (
        "wasi:http/types@0.3.0 resource implementation is missing"
    ):
        raise AssertionError("stable Spin final-WASI failure evidence drifted")
    if lock["spin_middleware_wasi_http"] == lock["wasi_http"]:
        raise AssertionError("remove the canary only after upstream supports final WASI HTTP")

    assert lock["spin_middleware_revision"] in workflow
    assert f'version: "{lock["wasmtime_version"]}"' in workflow
    assert f"wac-cli --version {lock['wac_cli_version']}" in workflow
    assert f"wasm-tools --version {lock['wasm_tools_version']}" in workflow

    audit_native_manifest(test_manifest, "/...")
    audit_native_manifest(counter_manifest, "/...")
    audit_composed_manifest(composed_test_manifest, "/...")
    audit_composed_manifest(composed_counter_manifest, "/...")

    asset_trigger = trigger_for(counter_manifest, "/pkg/...")
    if middleware_dependencies(asset_trigger):
        raise AssertionError("split-WASM assets must remain public")
    if asset_trigger["component"] == trigger_for(counter_manifest, "/...")["component"]:
        raise AssertionError("the public asset route must not expose the terminal app")
    if (
        counter_manifest["component"]["counter-pkg"]["source"]
        != stable_counter["component"]["counter-pkg"]["source"]
    ):
        raise AssertionError("the public asset server source or digest drifted")

    composed_asset_trigger = trigger_for(composed_counter_manifest, "/pkg/...")
    if middleware_dependencies(composed_asset_trigger):
        raise AssertionError("composed split-WASM assets must remain public")
    if (
        composed_counter_manifest["component"]["counter-pkg"]["source"]
        != stable_counter["component"]["counter-pkg"]["source"]
    ):
        raise AssertionError("the composed public asset server source or digest drifted")

    for trigger in stable_counter.get("trigger", {}).get("http", []):
        if middleware_dependencies(trigger):
            raise AssertionError("stable Spin manifests must not use experimental syntax")

    if args.composition_tools:
        resolve_tool("WAC_BIN", "wac", lock["wac_cli_version"])
        resolve_tool("WASM_TOOLS_BIN", "wasm-tools", lock["wasm_tools_version"])
    if args.spin:
        spin = os.environ.get("SPIN_BIN") or shutil.which("spin") or "spin"
        actual = command_version(spin)
        if lock["spin_stable_version"] not in actual:
            raise AssertionError("Spin behavioral runner must use stable Spin 4")
    if args.spin_native_canary:
        spin = os.environ.get("SPIN_BIN") or shutil.which("spin") or "spin"
        actual = command_version(spin)
        short_revision = lock["spin_middleware_revision"][:7]
        if short_revision not in actual:
            raise AssertionError(
                "Spin middleware runner does not match the pinned experimental revision"
            )
    if args.wasmtime:
        resolve_tool("WASMTIME_BIN", "wasmtime", lock["wasmtime_version"])

    print(
        "final-WASI middleware manifests are pinned; optional authn wraps the "
        "terminal apps; /pkg split assets remain public"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, KeyError, OSError, tomllib.TOMLDecodeError) as error:
        print(f"middleware manifest audit failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error

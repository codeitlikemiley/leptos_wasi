#!/usr/bin/env python3
"""Audit final-WASI middleware manifests, capabilities, and pinned tools."""

from __future__ import annotations

import argparse
import hashlib
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]
EXPECTED_STACK = ["request-id", "security-headers", "cors", "authn-policy"]
EXPECTED_AUTHZ_STACK = [*EXPECTED_STACK, "authz-http-pep"]
WASMTIME_DEPLOYMENT_CONTRACTS = {
    # These are deliberately code-side contracts, rather than fields copied
    # from deployment-policy.toml. The policy document declares what a release
    # deploys; this allowlist prevents a protected terminal from being silently
    # downgraded to authn-only or swapped for an unrelated guest by editing
    # that document alone.
    "wasmtime-authn": {
        "authentication_mode": "portable_component",
        "runtime": "wasmtime",
        "support": "experimental",
        "terminal_artifact": "tests/test-app-p3.wasm",
        "output_artifact": "tests/test-app-p3-middleware.wasm",
        "listener_count": 1,
        "middleware_profile": "authn",
        "middleware": EXPECTED_STACK,
        "artifact_sets": ["middleware"],
    },
    "wasmtime-authn-authz": {
        "authentication_mode": "portable_component",
        "runtime": "wasmtime",
        "support": "experimental",
        "terminal_artifact": (
            "tests/authz-fixture/target/wasm32-wasip2/release/"
            "leptos_wasi_authz_fixture.wasm"
        ),
        "output_artifact": (
            "examples/counter/target/wasm32-wasip2/release/"
            "counter-middleware.wasm"
        ),
        "listener_count": 1,
        "middleware_profile": "authn_authz",
        "middleware": EXPECTED_STACK,
        "artifact_sets": ["middleware", "authorization"],
    },
    "wasmtime-authn-authz-coarse": {
        "authentication_mode": "portable_component",
        "runtime": "wasmtime",
        "support": "experimental",
        "terminal_artifact": (
            "tests/authz-fixture/target/wasm32-wasip2/release/"
            "leptos_wasi_authz_fixture.wasm"
        ),
        "output_artifact": (
            "examples/counter/target/wasm32-wasip2/release/"
            "counter-middleware.wasm"
        ),
        "listener_count": 1,
        "middleware_profile": "authn_authz_coarse",
        "middleware": EXPECTED_AUTHZ_STACK,
        "artifact_sets": ["middleware", "authorization"],
    },
    "wasmtime-trusted-ingress-authz": {
        "authentication_mode": "trusted_ingress",
        "runtime": "wasmtime",
        "support": "reference",
        "terminal_artifact": (
            "tests/authz-fixture/target/wasm32-wasip2/release/"
            "leptos_wasi_authz_fixture.wasm"
        ),
        "output_artifact": (
            "tests/authz-fixture/target/wasm32-wasip2/release/"
            "leptos_wasi_authz_fixture.wasm"
        ),
        "listener_count": 1,
        "private_terminal": True,
        "edge_policy": "managed-private-ingress",
        "ingress_manifest": "tests/trusted-ingress/Cargo.toml",
        "network_descriptor": "tests/trusted-ingress/compose.yaml",
        "public_listener": "ingress",
        "terminal_listener": "private",
        "forwards_authorization": False,
        "allows_trusted_response_headers": False,
        "portable_fallback": False,
        "middleware_profile": "trusted_ingress_authz",
        "middleware": [],
        "artifact_sets": ["authorization"],
    },
}
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


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def audit_artifact_sets(lock: dict, policy: dict) -> dict[str, dict]:
    dirty_diagnostics = os.environ.get("AUTHZ_COMPANION_ALLOW_DIRTY") == "1"
    relative = policy.get("artifact_set_file")
    expected_digest = policy.get("artifact_set_sha256")
    if not isinstance(relative, str) or not relative:
        raise AssertionError("deployment policy must bind one artifact-set file")
    if not isinstance(expected_digest, str) or re.fullmatch(r"[0-9a-f]{64}", expected_digest) is None:
        raise AssertionError("deployment policy artifact-set digest must be SHA-256")
    path = ROOT / relative
    if not path.is_file() or sha256(path) != expected_digest:
        raise AssertionError("deployment policy artifact-set record drifted")
    with path.open("rb") as source:
        document = tomllib.load(source)
    if document.get("schema") != 1:
        raise AssertionError("artifact-set schema must be 1")
    bundles = document.get("bundle", [])
    if not isinstance(bundles, list):
        raise AssertionError("artifact bundles must be an array")
    if any(not isinstance(bundle, dict) for bundle in bundles):
        raise AssertionError("artifact bundles must be tables")
    by_id = {bundle.get("id"): bundle for bundle in bundles}
    if len(by_id) != len(bundles) or set(by_id) != {"middleware", "authorization"}:
        raise AssertionError("artifact sets must contain unique middleware and authorization bundles")

    for bundle_id, lock_section in (
        ("middleware", lock["middleware"]),
        ("authorization", lock["authorization"]),
    ):
        bundle = by_id[bundle_id]
        for bundle_key, lock_key in (
            ("name", "artifact_name" if bundle_id == "authorization" else "name"),
            (
                "version",
                "artifact_version" if bundle_id == "authorization" else "version",
            ),
            ("source_revision", "artifact_revision"),
        ):
            if bundle.get(bundle_key) != lock_section.get(lock_key):
                if dirty_diagnostics and bundle_id == "authorization":
                    continue
                raise AssertionError(
                    f"artifact bundle {bundle_id} {bundle_key} differs from compatibility lock"
                )
        for key in (
            "checksum_manifest",
            "provenance",
            "oci_manifest",
            "provenance_signature",
            "manifest_signature",
            "signing_key",
        ):
            digest = bundle.get(f"{key}_sha256")
            if not isinstance(bundle.get(key), str) or not bundle[key]:
                raise AssertionError(f"artifact bundle {bundle_id} lacks {key}")
            if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
                raise AssertionError(f"artifact bundle {bundle_id} has invalid {key} digest")

        artifacts = bundle.get("artifact", [])
        if not isinstance(artifacts, list):
            raise AssertionError(f"artifact bundle {bundle_id} entries must be an array")
        if any(not isinstance(artifact, dict) for artifact in artifacts):
            raise AssertionError(f"artifact bundle {bundle_id} entries must be tables")
        components = [artifact.get("component") for artifact in artifacts]
        if len(components) != len(set(components)):
            raise AssertionError(f"artifact bundle {bundle_id} repeats a component")
        if components != lock_section["components"]:
            raise AssertionError(
                f"artifact bundle {bundle_id} must exactly match locked components"
            )
        for artifact in artifacts:
            component = artifact["component"]
            if artifact.get("path") != f"components/{component}.wasm":
                raise AssertionError(f"artifact path drifted for {component}")
            for key in ("sha256", "sbom_sha256", "wit_sha256"):
                digest = artifact.get(key)
                if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
                    raise AssertionError(f"artifact {component} has invalid {key}")
            for key in ("sbom", "wit"):
                if not isinstance(artifact.get(key), str) or not artifact[key]:
                    raise AssertionError(f"artifact {component} lacks {key}")
    return by_id


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


def route_prefix(route: str) -> tuple[str, bool]:
    if route.endswith("..."):
        return route[:-3], True
    return route, False


def routes_overlap(left: str, right: str) -> bool:
    left_prefix, left_wildcard = route_prefix(left)
    right_prefix, right_wildcard = route_prefix(right)
    if not left_wildcard and not right_wildcard:
        return left == right
    if left_wildcard and right_wildcard:
        return left_prefix.startswith(right_prefix) or right_prefix.startswith(
            left_prefix
        )
    if left_wildcard:
        return right_prefix.startswith(left_prefix)
    return left_prefix.startswith(right_prefix)


def audit_wasmtime_deployment_contracts(profiles: list[dict]) -> None:
    """Bind each Wasmtime terminal to its immutable security profile.

    A generic non-empty middleware stack is insufficient for a guest whose
    server functions enforce domain authorization. The typed authorization
    fixture must retain its signed authorization artifact set; the coarse HTTP
    PEP is an explicit optional profile rather than a mandatory duplicate
    decision. Keeping both contracts here catches a downgrade or terminal
    swap before a deployment manifest is accepted.
    """
    by_name = {
        profile.get("name"): profile
        for profile in profiles
        if profile.get("runtime") == "wasmtime"
    }
    if set(by_name) != set(WASMTIME_DEPLOYMENT_CONTRACTS):
        raise AssertionError(
            "Wasmtime deployment profiles must exactly match the protected "
            "terminal allowlist"
        )
    for name, expected in WASMTIME_DEPLOYMENT_CONTRACTS.items():
        actual = by_name[name]
        for key, value in expected.items():
            if actual.get(key) != value:
                raise AssertionError(
                    f"Wasmtime deployment profile {name} drifted at {key}"
                )
        if actual.get("authentication_mode") == "trusted_ingress":
            for descriptor in ("ingress_manifest", "network_descriptor"):
                path = actual.get(descriptor)
                if not isinstance(path, str) or not (ROOT / path).is_file():
                    raise AssertionError(
                        f"trusted-ingress profile {name} lacks concrete {descriptor}"
                    )


def audit_deployment_policy(lock: dict) -> None:
    policy = load("tests/middleware/deployment-policy.toml")
    if policy.get("schema") != 2:
        raise AssertionError("deployment policy schema must be 2")
    spin_runtime = load("tests/spin/runtime-config.production.toml")
    outbound = spin_runtime.get("outbound_http", {})
    if outbound.get("connection_pooling") is not True:
        raise AssertionError("Spin production profile must pool outbound HTTP")
    concurrency = outbound.get("max_concurrent_requests")
    if not isinstance(concurrency, int) or not 1 <= concurrency <= 1000:
        raise AssertionError("Spin outbound concurrency must be explicitly bounded")
    profiles = policy.get("profile", [])
    if not isinstance(profiles, list):
        raise AssertionError("deployment profiles must be an array")
    if any(not isinstance(profile, dict) for profile in profiles):
        raise AssertionError("deployment profiles must be tables")
    blocked = [
        profile["name"]
        for profile in profiles
        if profile.get("runtime") == "spin"
        and profile.get("support") == "blocked_upstream"
    ]
    if lock.get("spin_final_wasi_supported") is False and not blocked:
        raise AssertionError("Spin final-WASI profiles must remain blocked upstream")
    if lock.get("spin_final_wasi_supported") is True and blocked:
        raise AssertionError(
            "Spin now supports final WASI; replace blocked canaries with promotion lanes"
        )
    names = [profile.get("name") for profile in profiles]
    if not names or len(names) != len(set(names)):
        raise AssertionError("deployment profile names must be unique")
    audit_wasmtime_deployment_contracts(profiles)
    bundles = audit_artifact_sets(lock, policy)
    support_allowlist = policy.get("support_allowlist")
    if not isinstance(support_allowlist, dict):
        raise AssertionError("deployment policy must declare a runtime support allowlist")

    declared_manifests: set[str] = set()
    middleware_components = set(lock["middleware"]["components"])
    authorization_components = set(lock["authorization"]["components"])
    allowed_components = middleware_components | authorization_components
    for profile in profiles:
        name = profile["name"]
        profile_kind = profile["middleware_profile"]
        expected = {
            "none": [],
            "authn": EXPECTED_STACK,
            "authn_authz": EXPECTED_STACK,
            "authn_authz_coarse": EXPECTED_AUTHZ_STACK,
            "trusted_ingress_authz": [],
        }.get(profile_kind)
        if expected is None or profile.get("middleware") != expected:
            raise AssertionError(f"deployment profile {name} has an invalid stack")
        expected_artifact_sets = {
            "none": [],
            "authn": ["middleware"],
            "authn_authz": ["middleware", "authorization"],
            "authn_authz_coarse": ["middleware", "authorization"],
            "trusted_ingress_authz": ["authorization"],
        }[profile_kind]
        if profile.get("artifact_sets") != expected_artifact_sets:
            raise AssertionError(
                f"deployment profile {name} has invalid artifact-set bindings"
            )
        declared_components = {
            artifact["component"]
            for bundle_id in expected_artifact_sets
            for artifact in bundles[bundle_id]["artifact"]
        }
        if not set(expected).issubset(declared_components):
            raise AssertionError(
                f"deployment profile {name} contains an unsigned component"
            )
        unknown = set(expected).difference(allowed_components)
        if unknown:
            raise AssertionError(
                f"deployment profile {name} uses unlocked components: {unknown}"
            )
        support = profile["support"]
        runtime = profile["runtime"]
        allowed_support = support_allowlist.get(runtime)
        if not isinstance(allowed_support, list) or support not in allowed_support:
            raise AssertionError(
                f"deployment profile {name} uses a disallowed runtime/support pair"
            )
        authentication_mode = profile.get("authentication_mode")
        expected_authentication_mode = {
            "none": "none",
            "authn": "portable_component",
            "authn_authz": "portable_component",
            "authn_authz_coarse": "portable_component",
            "trusted_ingress_authz": "trusted_ingress",
        }[profile_kind]
        if authentication_mode != expected_authentication_mode:
            raise AssertionError(
                f"deployment profile {name} has an invalid authentication mode"
            )
        if authentication_mode == "trusted_ingress" and (
            authentication_mode != "trusted_ingress"
            or profile.get("private_terminal") is not True
            or profile.get("edge_policy") != "managed-private-ingress"
        ):
            raise AssertionError(
                f"trusted-ingress profile {name} must require a private terminal"
            )

        if runtime == "wasmtime":
            if profile.get("listener_count") != 1:
                raise AssertionError(
                    f"Wasmtime profile {name} must expose exactly one app listener"
                )
            if (
                profile.get("output_artifact") == profile["terminal_artifact"]
                and authentication_mode != "trusted_ingress"
            ):
                raise AssertionError(
                    f"Wasmtime profile {name} must serve only a composed output"
                )
            continue
        if runtime != "spin":
            raise AssertionError(f"unknown deployment runtime in {name}: {runtime}")

        manifest_path = profile["manifest"]
        if manifest_path in declared_manifests:
            raise AssertionError(
                f"Spin manifest has more than one deployment profile: {manifest_path}"
            )
        declared_manifests.add(manifest_path)
        manifest = load(manifest_path)
        triggers = manifest.get("trigger", {}).get("http", [])
        routes = [trigger["route"] for trigger in triggers]
        if len(routes) != len(set(routes)):
            raise AssertionError(f"duplicate HTTP route in {manifest_path}")
        terminal_component = profile["terminal_component"]
        terminal_definition = manifest["component"][terminal_component]
        if terminal_definition.get("source") != profile["terminal_artifact"]:
            raise AssertionError(
                f"terminal artifact drifted for deployment profile {name}"
            )
        terminal_triggers = [
            trigger
            for trigger in triggers
            if trigger.get("component") == terminal_component
        ]
        if not terminal_triggers:
            raise AssertionError(f"profile {name} has no terminal HTTP trigger")

        exemptions = profile.get("public_exemption", [])
        exemption_keys = [
            (exemption["route"], exemption["component"])
            for exemption in exemptions
        ]
        if len(exemption_keys) != len(set(exemption_keys)):
            raise AssertionError(f"duplicate public exemption in {name}")
        overlaps: set[tuple[str, str]] = set()
        for terminal in terminal_triggers:
            for trigger in triggers:
                if trigger is terminal:
                    continue
                if routes_overlap(terminal["route"], trigger["route"]):
                    overlaps.add((trigger["route"], trigger["component"]))
        if overlaps != set(exemption_keys):
            raise AssertionError(
                f"route overlaps for {name} must exactly match public exemptions: "
                f"overlaps={sorted(overlaps)} exemptions={sorted(exemption_keys)}"
            )
        for exemption in exemptions:
            if exemption["component"] == terminal_component:
                raise AssertionError(f"public exemption reaches terminal in {name}")
            if not exemption.get("reason"):
                raise AssertionError(f"public exemption lacks a reason in {name}")
            source = manifest["component"][exemption["component"]].get("source")
            if not isinstance(source, dict):
                raise AssertionError(f"public exemption is not digest-pinned in {name}")
            if source.get("url") != exemption["source_url"] or source.get(
                "digest"
            ) != exemption["source_digest"]:
                raise AssertionError(f"public exemption source drifted in {name}")

    discovered = {
        path.relative_to(ROOT).as_posix()
        for path in ROOT.glob("**/spin*.toml")
        if "target" not in path.parts
    }
    if declared_manifests != discovered:
        raise AssertionError(
            "every Spin HTTP manifest must have exactly one deployment profile: "
            f"missing={sorted(discovered - declared_manifests)} "
            f"unknown={sorted(declared_manifests - discovered)}"
        )


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


def version_matches(expected: str, reported: str) -> bool:
    """Require a whole-version match rather than a substring one.

    A plain `in` test accepts a longer version that merely starts with the
    pinned one - `4.0.2` matches a reported `4.0.21` - which defeats the point
    of pinning exact tool revisions in `components.lock.toml`.
    """
    pattern = rf"(?<![0-9A-Za-z.-]){re.escape(expected)}(?![0-9A-Za-z.-])"
    return re.search(pattern, reported) is not None


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
    if not version_matches(version, actual):
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
    service_id = environment["WASI_MIDDLEWARE_SERVICE_ID"]
    expected_audiences = {f"api://{service_id}"}
    # The equality check already forces `audiences == {"api://" + service_id}`,
    # which no service ID can satisfy on its own, so the audience is distinct
    # from the service ID by construction rather than by a further assertion.
    if audiences != expected_audiences:
        raise AssertionError(
            f"authn audiences for {service_id} must be {expected_audiences}"
        )

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
    parser.add_argument("--spin-main", action="store_true")
    parser.add_argument("--spin-native-canary", action="store_true")
    parser.add_argument("--wasmtime", action="store_true")
    args = parser.parse_args()

    lock = load("tests/middleware/components.lock.toml")
    audit_deployment_policy(lock)
    authorization_lock = lock["authorization"]
    test_manifest = load("tests/spin-p3-middleware-vnext.toml")
    counter_manifest = load("examples/counter/spin.middleware-vnext.toml")
    composed_test_manifest = load("tests/spin-p3-middleware-composed.toml")
    composed_counter_manifest = load(
        "examples/counter/spin.middleware-composed.toml"
    )
    stable_counter = load("examples/counter/spin.toml")
    counter_cargo = load("examples/counter/Cargo.toml")
    test_app_cargo = load("tests/test-app/Cargo.toml")
    authz_fixture = load(authorization_lock["fixture_manifest"])
    workflow = (ROOT / ".github/workflows/main.yaml").read_text()
    root_manifest = load("Cargo.toml")

    if lock["wasi_http"] != "0.3.0" or lock["wasip3_version"] != "0.7.0":
        raise AssertionError("middleware integration must use final WASI HTTP 0.3")
    if lock["schema"] != 2:
        raise AssertionError("middleware compatibility lock schema must be 2")
    middleware_lock = lock["middleware"]
    if middleware_lock["version"] != "0.2.0-alpha.3":
        raise AssertionError("middleware integration must target the breaking 0.2 alpha")
    for key in ("baseline_revision", "source_revision", "artifact_revision"):
        if re.fullmatch(r"[0-9a-f]{40}", middleware_lock[key]) is None:
            raise AssertionError(f"middleware {key} must be a full Git revision")
    if middleware_lock["baseline_revision"] == middleware_lock["source_revision"]:
        raise AssertionError("middleware source revision must advance beyond its baseline")
    if authorization_lock["version"] != "0.1.0-alpha.4":
        raise AssertionError("authorization integration must target consolidated wasi-auth")
    for key in ("baseline_revision", "source_revision", "artifact_revision"):
        if re.fullmatch(r"[0-9a-f]{40}", authorization_lock[key]) is None:
            raise AssertionError(f"authorization {key} must be a full Git revision")
    if authorization_lock["baseline_revision"] == authorization_lock["source_revision"]:
        raise AssertionError("authorization source revision must advance beyond its baseline")
    if root_manifest["dependencies"]["wasip3"]["version"] != "=0.7.0":
        raise AssertionError("the library must pin wasip3 exactly")
    authz_packages = {
        authorization_lock["leptos_package"],
        authorization_lock["client_package"],
    }
    for name, manifest in (
        ("root crate", root_manifest),
        ("counter", counter_cargo),
        ("test app", test_app_cargo),
    ):
        unexpected = authz_packages.intersection(
            manifest.get("dependencies", {})
        )
        if unexpected:
            raise AssertionError(
                f"{name} must not depend on unpublished sibling packages: "
                f"{sorted(unexpected)}"
            )
    fixture_dependency = authz_fixture["dependencies"][
        authorization_lock["leptos_package"]
    ]
    if fixture_dependency["version"] != f'={authorization_lock["version"]}':
        raise AssertionError("authorization fixture must pin the exact alpha version")
    if not fixture_dependency["path"].endswith(
        authorization_lock["leptos_crate_path"]
    ):
        raise AssertionError("authorization fixture must use the locked sibling crate path")
    client_dependency = authz_fixture["dependencies"][
        authorization_lock["client_package"]
    ]
    if client_dependency["version"] != f'={authorization_lock["version"]}':
        raise AssertionError("authorization fixture must pin the exact client alpha")
    if not client_dependency["path"].endswith(
        authorization_lock["client_crate_path"]
    ):
        raise AssertionError("authorization fixture must use the locked client crate path")
    if set(client_dependency.get("features", [])) != {"wasip3"}:
        raise AssertionError("authorization fixture must use only the WASIp3 client transport")
    if authz_fixture["package"].get("publish") is not False:
        raise AssertionError("authorization fixture must never be publishable")
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
    if args.spin_main:
        spin = os.environ.get("SPIN_BIN") or shutil.which("spin") or "spin"
        actual = command_version(spin)
        short_revision = lock["spin_main_revision"][:7]
        if short_revision not in actual:
            raise AssertionError(
                "Spin main runner does not match the pinned compatibility revision"
            )
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
        "deployment policy is pinned; production uses a private trusted "
        "ingress; portable component middleware remains experimental"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, KeyError, OSError, tomllib.TOMLDecodeError) as error:
        print(f"middleware manifest audit failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error

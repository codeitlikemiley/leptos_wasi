#!/usr/bin/env python3
"""Verify one signed companion artifact set against the integration lock."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import subprocess
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]
LOCK = ROOT / "tests/middleware/components.lock.toml"
ARTIFACT_SETS = ROOT / "tests/middleware/artifact-sets.toml"


class ArtifactSetError(RuntimeError):
    """The declared artifact set is incomplete, inconsistent, or tampered."""


def sha256(path: pathlib.Path) -> str:
    """Return the SHA-256 digest for one file."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_digest(path: pathlib.Path, expected: str, label: str) -> None:
    """Require one file to exist and match its declared digest."""
    if not path.is_file():
        raise ArtifactSetError(f"missing {label}: {path}")
    actual = sha256(path)
    if actual != expected:
        raise ArtifactSetError(
            f"{label} digest mismatch: expected {expected}, found {actual}"
        )


def parse_checksums(path: pathlib.Path) -> dict[str, str]:
    """Parse a strict two-column SHA-256 manifest."""
    entries: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        checksum, separator, name = line.partition("  ")
        if (
            not separator
            or len(checksum) != 64
            or any(character not in "0123456789abcdef" for character in checksum)
            or not name
            or name in entries
        ):
            raise ArtifactSetError(f"invalid checksum manifest line: {line!r}")
        entries[name] = checksum
    if not entries:
        raise ArtifactSetError("artifact checksum manifest is empty")
    return entries


def validate_artifact_entries(
    bundle: dict[str, object], expected_components: list[str]
) -> list[dict[str, str]]:
    """Require exactly one declared artifact for every locked component."""
    raw_artifacts = bundle.get("artifact")
    if not isinstance(raw_artifacts, list):
        raise ArtifactSetError("bundle artifacts must be an array")
    artifacts: list[dict[str, str]] = []
    names: list[str] = []
    for raw in raw_artifacts:
        if not isinstance(raw, dict) or not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in raw.items()
        ):
            raise ArtifactSetError("artifact entries must contain string fields")
        artifact = dict(raw)
        artifacts.append(artifact)
        names.append(artifact.get("component", ""))
    if len(names) != len(set(names)):
        raise ArtifactSetError("artifact components must be unique")
    if names != expected_components:
        raise ArtifactSetError(
            "artifact components must exactly match the integration lock: "
            f"expected {expected_components}, found {names}"
        )
    return artifacts


def load_bundle(bundle_id: str) -> tuple[dict[str, object], dict[str, object]]:
    """Load one bundle and its matching compatibility section."""
    with LOCK.open("rb") as source:
        lock = tomllib.load(source)
    with ARTIFACT_SETS.open("rb") as source:
        document = tomllib.load(source)
    bundles = [bundle for bundle in document.get("bundle", []) if bundle.get("id") == bundle_id]
    if len(bundles) != 1:
        raise ArtifactSetError(f"expected one artifact bundle named {bundle_id}")
    section = {"middleware": "middleware", "authorization": "authorization"}.get(bundle_id)
    if section is None:
        raise ArtifactSetError(f"unknown artifact bundle: {bundle_id}")
    return bundles[0], lock[section]


def verify_bundle(
    bundle: dict[str, object],
    lock: dict[str, object],
    repository: pathlib.Path,
    cosign: pathlib.Path,
) -> None:
    """Verify content, provenance subjects, and both local signatures."""
    for key in ("name", "version"):
        if bundle.get(key) != lock.get(key):
            raise ArtifactSetError(f"artifact bundle {key} does not match compatibility lock")
    if bundle.get("source_revision") != lock.get("source_revision"):
        raise ArtifactSetError("artifact bundle source revision does not match compatibility lock")
    expected_components = lock.get("components")
    if not isinstance(expected_components, list) or not all(
        isinstance(component, str) for component in expected_components
    ):
        raise ArtifactSetError("compatibility component list is invalid")
    artifacts = validate_artifact_entries(bundle, expected_components)

    def declared_path(key: str) -> pathlib.Path:
        value = bundle.get(key)
        if not isinstance(value, str) or not value:
            raise ArtifactSetError(f"missing artifact bundle path: {key}")
        return repository / value

    def declared_digest(key: str) -> str:
        value = bundle.get(key)
        if not isinstance(value, str) or len(value) != 64:
            raise ArtifactSetError(f"invalid artifact bundle digest: {key}")
        return value

    checksums_path = declared_path("checksum_manifest")
    verify_digest(
        checksums_path,
        declared_digest("checksum_manifest_sha256"),
        "checksum manifest",
    )
    checksums = parse_checksums(checksums_path)
    for name, checksum in checksums.items():
        verify_digest(repository / "artifacts" / name, checksum, f"artifact {name}")

    production_entries = {
        name: checksum
        for name, checksum in checksums.items()
        if name.startswith("components/") and name.endswith(".wasm")
    }
    declared_production = {
        artifact["path"]: artifact["sha256"] for artifact in artifacts
    }
    if production_entries != declared_production:
        raise ArtifactSetError(
            "production artifact entries differ from the signed checksum manifest"
        )

    for artifact in artifacts:
        for path_key, digest_key, label in (
            ("sbom", "sbom_sha256", "component SBOM"),
            ("wit", "wit_sha256", "component WIT report"),
        ):
            path = artifact.get(path_key)
            digest = artifact.get(digest_key)
            if not path or not digest:
                raise ArtifactSetError(f"missing {label} metadata for {artifact['component']}")
            verify_digest(repository / path, digest, f"{label} {artifact['component']}")

    provenance = declared_path("provenance")
    verify_digest(provenance, declared_digest("provenance_sha256"), "provenance")
    statement = json.loads(provenance.read_text(encoding="utf-8"))
    subjects = {
        subject["name"]: subject["digest"]["sha256"]
        for subject in statement.get("subject", [])
    }
    extra_subjects = bundle.get("attested_input", [])
    if not isinstance(extra_subjects, list):
        raise ArtifactSetError("attested inputs must be an array")
    expected_subjects = dict(checksums)
    for entry in extra_subjects:
        if not isinstance(entry, dict):
            raise ArtifactSetError("attested input entries must be tables")
        name = entry.get("path")
        digest = entry.get("sha256")
        source = entry.get("source")
        if not all(isinstance(value, str) and value for value in (name, digest, source)):
            raise ArtifactSetError("attested input fields are invalid")
        verify_digest(repository / source, digest, f"attested input {name}")
        expected_subjects[name] = digest
    if subjects != expected_subjects:
        raise ArtifactSetError("provenance subjects differ from the declared artifact set")

    manifest = declared_path("oci_manifest")
    provenance_signature = declared_path("provenance_signature")
    manifest_signature = declared_path("manifest_signature")
    signing_key = declared_path("signing_key")
    for path, key, label in (
        (manifest, "oci_manifest_sha256", "OCI manifest"),
        (provenance_signature, "provenance_signature_sha256", "provenance signature"),
        (manifest_signature, "manifest_signature_sha256", "manifest signature"),
        (signing_key, "signing_key_sha256", "signing key"),
    ):
        verify_digest(path, declared_digest(key), label)

    for payload, signature in (
        (provenance, provenance_signature),
        (manifest, manifest_signature),
    ):
        subprocess.run(
            [
                str(cosign),
                "verify-blob",
                "--key",
                str(signing_key),
                "--bundle",
                str(signature),
                "--insecure-ignore-tlog",
                str(payload),
            ],
            check=True,
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", choices=("middleware", "authorization"))
    parser.add_argument("--repository", required=True, type=pathlib.Path)
    parser.add_argument("--cosign", required=True, type=pathlib.Path)
    args = parser.parse_args()
    bundle, lock = load_bundle(args.bundle)
    verify_bundle(bundle, lock, args.repository.resolve(), args.cosign)
    print(f"verified signed artifact set: {args.bundle}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ArtifactSetError, KeyError, OSError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"artifact-set verification failed: {error}") from error

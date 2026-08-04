#!/usr/bin/env bash

set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/authz-common.sh"

ROOT="${LEPTOS_WASI_ROOT}"
repository="$(authz_repository)"

[[ -f "${repository}/Cargo.toml" ]] || {
  echo "wasi-auth checkout is unavailable: ${repository}" >&2
  exit 2
}

expected_version="$(authz_lock_value version)"
actual_version="$(python3 - "${repository}/Cargo.toml" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    print(tomllib.load(source)["workspace"]["package"]["version"])
PY
)"
[[ "${actual_version}" == "${expected_version}" ]] || {
  echo "wasi-auth version mismatch: expected ${expected_version}, found ${actual_version}" >&2
  exit 1
}

expected_revision="$(authz_lock_value source_revision)"
actual_revision="$(git -C "${repository}" rev-parse HEAD)"
[[ "${actual_revision}" == "${expected_revision}" ]] || {
  echo "wasi-auth source revision mismatch: expected ${expected_revision}, found ${actual_revision}" >&2
  exit 1
}

baseline_revision="$(authz_lock_value baseline_revision)"
git -C "${repository}" merge-base --is-ancestor "${baseline_revision}" "${actual_revision}" || {
  echo "wasi-auth source does not descend from baseline ${baseline_revision}" >&2
  exit 1
}

if [[ -n "$(git -C "${repository}" status --porcelain)" ]]; then
  if [[ "${AUTHZ_COMPANION_ALLOW_DIRTY:-0}" != "1" ]]; then
    echo "wasi-auth checkout must be clean before fixture verification" >&2
    exit 1
  fi
  echo "warning: using dirty wasi-auth source for non-release local diagnostics" >&2
fi

companion_manifest="${repository}/$(authz_lock_value companion_manifest)"
[[ -f "${companion_manifest}" ]] || {
  echo "companion surface manifest is unavailable: ${companion_manifest}" >&2
  exit 2
}
# The digest catches any drift; the assertions after it name what drifted,
# which a consumer would otherwise meet as a resolution failure inside a
# fixture build.
python3 - "${companion_manifest}" "${AUTHZ_LOCK}" <<'PY'
import hashlib
import pathlib
import sys
import tomllib

manifest = pathlib.Path(sys.argv[1])
with open(sys.argv[2], "rb") as source:
    lock = tomllib.load(source)
authorization = lock["authorization"]
middleware = lock["middleware"]

actual = hashlib.sha256(manifest.read_bytes()).hexdigest()
expected = authorization["companion_manifest_sha256"]
if actual != expected:
    raise SystemExit(
        f"companion surface manifest drifted: expected {expected}, found {actual}"
    )

companion = tomllib.loads(manifest.read_text(encoding="utf-8"))

if companion.get("schema") != 1:
    raise SystemExit("companion surface manifest schema must be 1")

artifact = companion["artifact"]
for manifest_key, lock_key in (
    ("name", "artifact_name"),
    ("version", "artifact_version"),
):
    if artifact.get(manifest_key) != authorization.get(lock_key):
        raise SystemExit(
            f"companion artifact {manifest_key} disagrees with the lock: "
            f"{artifact.get(manifest_key)} != {authorization.get(lock_key)}"
        )
if artifact.get("components") != authorization["components"]:
    raise SystemExit(
        "companion artifact components disagree with the lock: "
        f"{artifact.get('components')} != {authorization['components']}"
    )

# Every crate a consumer is supported in naming must sit on the version line
# its own workspace is pinned to. The legacy workspace has an independent one.
expected = {
    "wasi-auth": authorization["version"],
    "wasi-http-middleware": middleware["version"],
}
direct = [crate for crate in companion["crate"] if crate.get("direct")]
if not direct:
    raise SystemExit("companion surface manifest records no directly usable crates")
for crate in direct:
    workspace = crate.get("workspace")
    if workspace not in expected:
        raise SystemExit(
            f"companion crate {crate.get('package')} names an unlocked workspace: {workspace}"
        )
    if crate.get("version") != expected[workspace]:
        raise SystemExit(
            f"companion crate {crate.get('package')} is {crate.get('version')}, "
            f"but {workspace} is locked at {expected[workspace]}"
        )
for package in (authorization["leptos_package"], authorization["client_package"]):
    if not any(crate.get("package") == package for crate in direct):
        raise SystemExit(
            f"companion surface manifest does not record the locked package {package}"
        )
print(f"verified {len(direct)} directly usable companion crates against the lock")
PY

crate_path="${repository}/$(authz_lock_value leptos_crate_path)"
[[ -f "${crate_path}/Cargo.toml" ]] || {
  echo "Leptos authorization package is unavailable: ${crate_path}" >&2
  exit 2
}

fixture_manifest="${ROOT}/$(authz_lock_value fixture_manifest)"
[[ -f "${fixture_manifest}" ]] || {
  echo "Leptos authorization fixture is unavailable: ${fixture_manifest}" >&2
  exit 2
}

cargo clippy --locked --manifest-path "${fixture_manifest}" \
  --target wasm32-wasip2 --all-features -- -D warnings

echo "verified private leptos-wasi-authz compatibility package ${expected_version} at ${actual_revision}"

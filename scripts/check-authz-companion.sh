#!/usr/bin/env bash

set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/authz-common.sh"

ROOT="${LEPTOS_WASI_ROOT}"
repository="$(authz_repository)"

[[ -f "${repository}/Cargo.toml" ]] || {
  echo "wasi-authz checkout is unavailable: ${repository}" >&2
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
  echo "wasi-authz version mismatch: expected ${expected_version}, found ${actual_version}" >&2
  exit 1
}

expected_revision="$(authz_lock_value source_revision)"
actual_revision="$(git -C "${repository}" rev-parse HEAD)"
[[ "${actual_revision}" == "${expected_revision}" ]] || {
  echo "wasi-authz source revision mismatch: expected ${expected_revision}, found ${actual_revision}" >&2
  exit 1
}

baseline_revision="$(authz_lock_value baseline_revision)"
git -C "${repository}" merge-base --is-ancestor "${baseline_revision}" "${actual_revision}" || {
  echo "wasi-authz source does not descend from baseline ${baseline_revision}" >&2
  exit 1
}

if [[ -n "$(git -C "${repository}" status --porcelain)" ]]; then
  echo "wasi-authz checkout must be clean before fixture verification" >&2
  exit 1
fi

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

echo "verified leptos-wasi-authz ${expected_version} at ${actual_revision}"

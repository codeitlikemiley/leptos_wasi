#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
baseline_revision="${LEPTOS_WASI_SEMVER_BASELINE:-fa5a983}"
baseline_root="$(mktemp -d "${TMPDIR:-/tmp}/leptos-wasi-semver.XXXXXX")"
trap 'rm -rf -- "${baseline_root}"' EXIT

git -C "${repo_root}" archive "${baseline_revision}" \
  | tar -x -C "${baseline_root}"

# `leptos-wasi-runtime` is the crates.io publication fallback for the existing
# `leptos_wasi` library. Normalize only the baseline package metadata so
# cargo-semver-checks compares the same public Rust crate and API surface.
perl -0pi -e '
  s/name = "leptos_wasi"/name = "leptos-wasi-runtime"/;
  s/\n\[dependencies\]/\n[lib]\nname = "leptos_wasi"\n\n[dependencies]/;
' "${baseline_root}/Cargo.toml"

cargo semver-checks check-release \
  --manifest-path "${repo_root}/Cargo.toml" \
  --baseline-root "${baseline_root}" \
  --release-type patch

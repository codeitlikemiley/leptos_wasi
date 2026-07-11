#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

mode="${1:-default}"
case "${mode}" in
  default) suffix=""; cargo_features=() ;;
  no-default-features)
    suffix="-no-default-features"
    cargo_features=(--no-default-features)
    ;;
  *)
    echo "usage: $0 [default|no-default-features]" >&2
    exit 2
    ;;
esac

repository="$(middleware_lock_value spin_middleware_repository)"
revision="$(middleware_lock_value spin_main_revision)"
destination="${ROOT}/target/tools/spin-${revision}${suffix}"
source_directory="${destination}/source"
binary="${destination}/spin"

if [[ -x "${binary}" ]] && "${binary}" --version 2>&1 | grep -Fq "${revision:0:7}"; then
  printf '%s\n' "${binary}"
  exit 0
fi

rm -rf "${destination}"
mkdir -p "${destination}"
git clone --filter=blob:none --no-checkout "${repository}" "${source_directory}"
git -C "${source_directory}" fetch --depth 1 origin "${revision}"
git -C "${source_directory}" checkout --detach "${revision}"
[[ "$(git -C "${source_directory}" rev-parse HEAD)" == "${revision}" ]]
cargo build \
  --locked \
  --release \
  --bin spin \
  "${cargo_features[@]}" \
  --manifest-path "${source_directory}/Cargo.toml"
install -m 0755 "${source_directory}/target/release/spin" "${binary}"
"${binary}" --version 2>&1 | grep -Fq "${revision:0:7}"
printf '%s\n' "${binary}"

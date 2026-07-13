#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/authz-common.sh"

repository="$(authz_repository)"
[[ -x "${repository}/scripts/with-spicedb-pdp-wasmtime.sh" ]] || {
  echo "SpiceDB PDP harness is unavailable: ${repository}" >&2
  exit 2
}

bash "${repository}/scripts/build-components.sh"
bash "${repository}/scripts/generate-checksums.sh"
bash "${repository}/scripts/check-component-contracts.sh"
bash "${repository}/scripts/with-spicedb-pdp-wasmtime.sh" \
  env \
  AUTHZ_TEST_ONLY=1 \
  AUTHZ=1 \
  MIDDLEWARE=1 \
  HOST=wasmtime \
  "${ROOT}/tests/browser/run.sh"

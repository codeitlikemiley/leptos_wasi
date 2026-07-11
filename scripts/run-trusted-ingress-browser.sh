#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/authz-common.sh"

repository="$(authz_repository)"
[[ -x "${repository}/scripts/with-spicedb-pdp-wasmtime.sh" ]] || {
  echo "SpiceDB PDP harness is unavailable: ${repository}" >&2
  exit 2
}

WASI_AUTHZ_START_COMPATIBILITY_PDP=0 \
bash "${repository}/scripts/with-spicedb-pdp-wasmtime.sh" \
  env \
  AUTHZ_TEST_ONLY=1 \
  AUTHZ=1 \
  MIDDLEWARE=0 \
  AUTHENTICATION_MODE=trusted_ingress \
  HOST=wasmtime \
  "${ROOT}/tests/browser/run.sh"

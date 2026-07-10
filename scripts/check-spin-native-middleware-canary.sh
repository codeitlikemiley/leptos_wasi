#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"
SPIN_SOURCE_DIR="${1:?usage: $0 SPIN_SOURCE_DIR}"
EXPECTED_REVISION="$(middleware_lock_value spin_middleware_revision)"
EXPECTED_WIT="$(middleware_lock_value spin_middleware_wasi_http)"

actual_revision="$(git -C "$SPIN_SOURCE_DIR" rev-parse HEAD)"
[[ "$actual_revision" == "$EXPECTED_REVISION" ]] || {
  echo "Spin canary revision mismatch: $actual_revision" >&2
  exit 1
}

rg -q "MW_HANDLER_INTERFACE.*wasi:http/handler@$EXPECTED_WIT" "$SPIN_SOURCE_DIR" || {
  echo "Spin middleware ABI changed; reassess final-WASI native support" >&2
  exit 1
}

if rg -q 'MW_HANDLER_INTERFACE.*wasi:http/handler@0\.3\.0[";]' "$SPIN_SOURCE_DIR"; then
  echo "Spin now advertises final WASI HTTP; remove the expected-incompatibility canary" >&2
  exit 1
fi

echo "expected incompatibility confirmed: pinned Spin middleware imports wasi:http/handler@$EXPECTED_WIT"

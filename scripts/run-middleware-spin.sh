#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"
PORT="${PORT:-3000}"

"$ROOT/scripts/audit-middleware-manifests.py" --spin --composition-tools
"$ROOT/scripts/build-middleware-test-components.sh"
"$ROOT/scripts/compose-middleware.sh" \
  "$ROOT/tests/test-app-p3.wasm" \
  "$ROOT/tests/test-app-p3-middleware.wasm" \
  "$ROOT/tests/middleware-artifacts/request-id.wasm" \
  "$ROOT/tests/middleware-artifacts/security-headers.wasm" \
  "$ROOT/tests/middleware-artifacts/cors.wasm" \
  "$ROOT/tests/middleware-artifacts/authn-policy.wasm"

PORT="$PORT" exec "$ROOT/scripts/check-spin-final-wasi-canary.sh" \
  "$ROOT/tests/spin-p3-middleware-composed.toml"

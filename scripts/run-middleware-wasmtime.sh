#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"
PORT="${PORT:-3000}"

"$ROOT/scripts/audit-middleware-manifests.py" --wasmtime --composition-tools
"$ROOT/scripts/build-middleware-test-components.sh"
"$ROOT/scripts/compose-middleware.sh" \
  "$ROOT/tests/test-app-p3.wasm" \
  "$ROOT/tests/test-app-p3-middleware.wasm" \
  "$ROOT/tests/middleware-artifacts/request-id.wasm" \
  "$ROOT/tests/middleware-artifacts/security-headers.wasm" \
  "$ROOT/tests/middleware-artifacts/cors.wasm" \
  "$ROOT/tests/middleware-artifacts/authn-policy.wasm"

WASMTIME_BIN="$(resolve_middleware_tool WASMTIME_BIN wasmtime "$(middleware_lock_value wasmtime_version)")"
exec "$WASMTIME_BIN" serve \
  -W component-model-async=y \
  -S p3=y \
  -S cli=y \
  -S http=y \
  -S inherit-network=y \
  --env=WASI_MIDDLEWARE_CORS_ORIGINS=http://127.0.0.1:$PORT \
  --env=WASI_MIDDLEWARE_CORS_METHODS=GET,HEAD,POST \
  --env=WASI_MIDDLEWARE_CORS_HEADERS=content-type,authorization,x-request-id \
  --env=WASI_MIDDLEWARE_CORS_ALLOW_CREDENTIALS=false \
  --env=WASI_MIDDLEWARE_AUTHN_BROKER_URL=http://127.0.0.1:3112/authenticate \
  --env=WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS=2000 \
  --env=WASI_MIDDLEWARE_AUTHN_MODE=optional \
  --env=WASI_MIDDLEWARE_SERVICE_ID=leptos-wasi-test-app \
  --env=WASI_MIDDLEWARE_AUTHN_AUDIENCES=leptos-wasi-test-app \
  --env=WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT=64 \
  --env=WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK=true \
  --dir="$ROOT/tests/test-app/static::/static" \
  --addr "127.0.0.1:$PORT" \
  "$ROOT/tests/test-app-p3-middleware.wasm"

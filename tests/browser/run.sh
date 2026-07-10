#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"
HOST="${HOST:-wasmtime}"
PORT="${PORT:-3000}"
MIDDLEWARE="${MIDDLEWARE:-0}"

cd "$ROOT/examples/counter"
cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
LEPTOS_OUTPUT_NAME=counter cargo build \
  --locked \
  --lib \
  --target wasm32-wasip2 \
  --release \
  --no-default-features \
  --features ssr
make verify-split

SERVER_COMPONENT="target/wasm32-wasip2/release/counter.wasm"
if [[ "$MIDDLEWARE" == "1" ]]; then
  "$ROOT/scripts/audit-middleware-manifests.py"
  "$ROOT/scripts/sync-middleware-components.sh"
  SERVER_COMPONENT="target/wasm32-wasip2/release/counter-middleware.wasm"
  "$ROOT/scripts/compose-middleware.sh" \
    "target/wasm32-wasip2/release/counter.wasm" \
    "$SERVER_COMPONENT" \
    "$ROOT/tests/middleware-artifacts/request-id.wasm" \
    "$ROOT/tests/middleware-artifacts/security-headers.wasm" \
    "$ROOT/tests/middleware-artifacts/cors.wasm" \
    "$ROOT/tests/middleware-artifacts/authn-policy.wasm"
fi

if [[ "$HOST" == "spin" ]]; then
  if [[ "$MIDDLEWARE" == "1" ]]; then
    "$ROOT/scripts/audit-middleware-manifests.py" --spin
    manifest="$PWD/spin.middleware-composed.toml"
  else
    manifest="$PWD/spin.toml"
  fi
  PORT="$PORT" "$ROOT/scripts/check-spin-final-wasi-canary.sh" "$manifest"
  echo "Spin browser E2E is blocked until a tagged runtime links final wasi:http@0.3.0"
  exit 0
fi

WASMTIME_BIN="$(resolve_middleware_tool WASMTIME_BIN wasmtime "$(middleware_lock_value wasmtime_version)")"
BROKER_PID=""
if [[ "$MIDDLEWARE" == "1" ]]; then
  "$WASMTIME_BIN" serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --addr "127.0.0.1:3112" \
    "$ROOT/tests/middleware-artifacts/mock-authn-broker.wasm" \
    >"$ROOT/tests/browser/authn-broker.log" 2>&1 &
  BROKER_PID=$!
  for _ in $(seq 1 100); do
    status="$(curl --silent --max-time 1 --request POST --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:3112/authenticate" || true)"
    [[ "$status" == "401" ]] && break
    sleep 0.05
  done
  [[ "${status:-000}" == "401" ]]
fi

case "$HOST" in
  wasmtime)
    if [[ "$MIDDLEWARE" == "1" ]]; then
      "$ROOT/scripts/audit-middleware-manifests.py" --wasmtime
    fi
    MIDDLEWARE_ENV=()
    if [[ "$MIDDLEWARE" == "1" ]]; then
      MIDDLEWARE_ENV=(
        -S inherit-network=y
        --env=WASI_MIDDLEWARE_CORS_ORIGINS=http://127.0.0.1:3110,http://127.0.0.1:3111
        --env=WASI_MIDDLEWARE_CORS_METHODS=GET,HEAD,POST
        --env=WASI_MIDDLEWARE_CORS_HEADERS=content-type,authorization,x-request-id
        --env=WASI_MIDDLEWARE_CORS_ALLOW_CREDENTIALS=false
        --env=WASI_MIDDLEWARE_AUTHN_BROKER_URL=http://127.0.0.1:3112/authenticate
        --env=WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS=2000
        --env=WASI_MIDDLEWARE_AUTHN_MODE=optional
        --env=WASI_MIDDLEWARE_SERVICE_ID=leptos-wasi-counter
        --env=WASI_MIDDLEWARE_AUTHN_AUDIENCES=leptos-wasi-counter
        --env=WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT=64
        --env=WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK=true
      )
    fi
    "$WASMTIME_BIN" serve \
      -W component-model-async=y \
      -S p3=y \
      -S cli=y \
      -S http=y \
      "${MIDDLEWARE_ENV[@]}" \
      --dir="$PWD/target/site::/site" \
      --env=LEPTOS_OUTPUT_NAME=counter \
      --env=LEPTOS_SITE_ROOT=/site \
      --env=LEPTOS_SITE_PKG_DIR=pkg \
      --addr "127.0.0.1:$PORT" \
      "$SERVER_COMPONENT" &
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

SERVER_PID=$!
cleanup() {
  kill "$SERVER_PID" 2>/dev/null || true
  [[ -n "$BROKER_PID" ]] && kill "$BROKER_PID" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 60); do
  if curl --fail --silent "http://127.0.0.1:$PORT/" >/dev/null; then
    break
  fi
  sleep 1
done
curl --fail --silent "http://127.0.0.1:$PORT/" >/dev/null

cd "$ROOT/tests/browser"
if [[ ! -x node_modules/.bin/playwright ]]; then
  npm ci
fi
BASE_URL="http://127.0.0.1:$PORT" MIDDLEWARE="$MIDDLEWARE" npm test

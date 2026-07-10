#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"
HOST="${HOST:-all}"
cd "$ROOT"

./scripts/audit-middleware-manifests.py
./scripts/build-middleware-test-components.sh
./scripts/compose-middleware.sh \
  tests/test-app-p3.wasm \
  tests/test-app-p3-middleware.wasm \
  tests/middleware-artifacts/request-id.wasm \
  tests/middleware-artifacts/security-headers.wasm \
  tests/middleware-artifacts/cors.wasm \
  tests/middleware-artifacts/authn-policy.wasm

case "$HOST" in
  all)
    ./scripts/audit-middleware-manifests.py --spin --wasmtime
    ./scripts/check-spin-final-wasi-canary.sh \
      "$ROOT/tests/spin-p3-middleware-composed.toml"
    ;;
  wasmtime)
    ./scripts/audit-middleware-manifests.py --wasmtime
    ;;
  spin)
    ./scripts/audit-middleware-manifests.py --spin
    ./scripts/check-spin-final-wasi-canary.sh \
      "$ROOT/tests/spin-p3-middleware-composed.toml"
    echo "Spin middleware remains a compatibility canary; Wasmtime is the blocking final-WASI runtime"
    exit 0
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

WASMTIME_BIN="$(resolve_middleware_tool WASMTIME_BIN wasmtime "$(middleware_lock_value wasmtime_version)")"
export WASMTIME_BIN

BROKER_PORT="${BROKER_PORT:-3112}"
export WASI_MIDDLEWARE_AUTHN_BROKER_URL="http://127.0.0.1:${BROKER_PORT}/authenticate"
BROKER_LOG="$(mktemp -t leptos-wasi-authn-broker.XXXXXX)"
"$WASMTIME_BIN" serve \
  -W component-model-async=y \
  -S p3=y \
  -S cli=y \
  -S http=y \
  --addr "127.0.0.1:$BROKER_PORT" \
  tests/middleware-artifacts/mock-authn-broker.wasm \
  >"$BROKER_LOG" 2>&1 &
BROKER_PID=$!
cleanup() {
  status=$?
  kill "$BROKER_PID" 2>/dev/null || true
  wait "$BROKER_PID" 2>/dev/null || true
  if [[ $status -ne 0 ]]; then
    cat "$BROKER_LOG" >&2
  fi
  rm -f "$BROKER_LOG"
  exit "$status"
}
trap cleanup EXIT INT TERM

for _ in $(seq 1 100); do
  status="$(curl --silent --max-time 1 --request POST --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:$BROKER_PORT/authenticate" || true)"
  [[ "$status" == "401" ]] && break
  sleep 0.05
done
[[ "${status:-000}" == "401" ]] || {
  echo "mock authentication broker did not become ready" >&2
  exit 1
}

cargo test --locked --test e2e experimental_middleware_wasmtime_wasip3 \
  -- --ignored --nocapture --test-threads=1

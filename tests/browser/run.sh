#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"

available_port() {
  python3 -c 'import socket; stream=socket.socket(); stream.bind(("127.0.0.1", 0)); print(stream.getsockname()[1]); stream.close()'
}

HOST="${HOST:-wasmtime}"
PORT="${PORT:-$(available_port)}"
MIDDLEWARE="${MIDDLEWARE:-0}"
AUTHZ="${AUTHZ:-0}"
AUTHZ_PDP_TOKEN="${AUTHZ_PDP_TOKEN:-leptos-wasi-browser-pdp-token-do-not-log}"
SPICEDB_PDP_URL="${WASI_AUTHZ_TEST_PDP_URL:-}"
SPICEDB_PDP_TOKEN="${WASI_AUTHZ_TEST_PDP_BEARER_TOKEN:-}"
AUTHZ_FULL_CHAIN_BENCHMARK="${AUTHZ_FULL_CHAIN_BENCHMARK:-0}"
AUTHZ_FULL_CHAIN_BENCHMARK_ONLY="${AUTHZ_FULL_CHAIN_BENCHMARK_ONLY:-0}"
AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS:-5000}"
AUTHZ_FULL_CHAIN_SOAK="${AUTHZ_FULL_CHAIN_SOAK:-0}"
AUTHZ_FULL_CHAIN_SOAK_DURATION="${AUTHZ_FULL_CHAIN_SOAK_DURATION:-600}"
# Keep the normal fixture admission bound explicit, while allowing dedicated
# diagnostics to select another validated per-instance limit.
AUTHN_MAX_IN_FLIGHT="${AUTHN_MAX_IN_FLIGHT:-128}"

if [[ "$AUTHZ" == "1" && "$MIDDLEWARE" != "1" ]]; then
  echo "AUTHZ=1 requires the trusted MIDDLEWARE=1 boundary" >&2
  exit 2
fi
if [[ "$AUTHZ" == "1" && ( -z "$SPICEDB_PDP_URL" || -z "$SPICEDB_PDP_TOKEN" ) ]]; then
  echo "AUTHZ=1 must run through scripts/run-authz-browser.sh" >&2
  exit 2
fi
if [[ "$AUTHZ" == "1" ]]; then
  [[ "${AUTHZ_TEST_ONLY:-}" == "1" ]] || {
    echo "AUTHZ=1 browser runs are test-only; use scripts/run-authz-browser.sh" >&2
    exit 2
  }
  case "$SPICEDB_PDP_URL" in
    http://127.0.0.1:*|http://localhost:*) ;;
    *)
      echo "AUTHZ=1 browser runs require a loopback SpiceDB test PDP" >&2
      exit 2
      ;;
  esac
  [[ "$AUTHZ_PDP_TOKEN" == "leptos-wasi-browser-pdp-token-do-not-log" ]] || {
    echo "AUTHZ=1 browser runs refuse a non-fixture Cedar PDP credential" >&2
    exit 2
  }
  [[ "$SPICEDB_PDP_TOKEN" == pep-component-live-secret-* ]] || {
    echo "AUTHZ=1 browser runs refuse a non-fixture SpiceDB PDP credential" >&2
    exit 2
  }
fi

cd "$ROOT/examples/counter"
cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
if [[ "$AUTHZ" != "1" ]]; then
  LEPTOS_OUTPUT_NAME=counter cargo build \
    --locked \
    --lib \
    --target wasm32-wasip2 \
    --release \
    --no-default-features \
    --features ssr
fi
make verify-split

SERVER_COMPONENT="target/wasm32-wasip2/release/counter.wasm"
if [[ "$AUTHZ" == "1" ]]; then
  "$ROOT/scripts/sync-authz-components.sh"
  LEPTOS_OUTPUT_NAME=counter cargo build \
    --locked \
    --manifest-path "$ROOT/tests/authz-fixture/Cargo.toml" \
    --target wasm32-wasip2 \
    --release \
    --all-features
  SERVER_COMPONENT="$ROOT/tests/authz-fixture/target/wasm32-wasip2/release/leptos_wasi_authz_fixture.wasm"
fi
if [[ "$MIDDLEWARE" == "1" ]]; then
  "$ROOT/scripts/audit-middleware-manifests.py"
  "$ROOT/scripts/sync-middleware-components.sh"
  TERMINAL_COMPONENT="$SERVER_COMPONENT"
  SERVER_COMPONENT="target/wasm32-wasip2/release/counter-middleware.wasm"
  DEPLOYMENT_PROFILE="wasmtime-authn"
  if [[ "$AUTHZ" == "1" ]]; then
    DEPLOYMENT_PROFILE="wasmtime-authn-authz"
  fi
  MIDDLEWARE_COMPONENTS=()
  while IFS= read -r component; do
    if [[ "$component" == "authz-http-pep" ]]; then
      MIDDLEWARE_COMPONENTS+=(
        "$ROOT/tests/authz-artifacts/$component.wasm"
      )
    else
      MIDDLEWARE_COMPONENTS+=(
        "$ROOT/tests/middleware-artifacts/$component.wasm"
      )
    fi
  done < <(
    "$ROOT/scripts/deployment-profile-components.py" \
      "$DEPLOYMENT_PROFILE"
  )
  "$ROOT/scripts/compose-middleware.sh" \
    "$TERMINAL_COMPONENT" \
    "$SERVER_COMPONENT" \
    "${MIDDLEWARE_COMPONENTS[@]}"
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
BROKER_PORT="${BROKER_PORT:-$(available_port)}"
CEDAR_PDP_PORT="${CEDAR_PDP_PORT:-$(available_port)}"
BROKER_PID=""
PDP_PID=""
SERVER_PID=""
APP_LOG="$ROOT/tests/browser/app.log"
PDP_LOG="$ROOT/tests/browser/cedar-pdp.log"
BROKER_LOG="$ROOT/tests/browser/authn-broker.log"
rm -f "$APP_LOG" "$PDP_LOG" "$BROKER_LOG"
LOG_FILES=("$APP_LOG")
cleanup() {
  status=$?
  trap - EXIT
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  [[ -n "$BROKER_PID" ]] && kill "$BROKER_PID" 2>/dev/null || true
  [[ -n "$PDP_PID" ]] && kill "$PDP_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
  [[ -n "$BROKER_PID" ]] && wait "$BROKER_PID" 2>/dev/null || true
  [[ -n "$PDP_PID" ]] && wait "$PDP_PID" 2>/dev/null || true
  for sentinel in \
    "$AUTHZ_PDP_TOKEN" \
    "$SPICEDB_PDP_TOKEN" \
    "Bearer allow" \
    "Bearer readonly" \
    "Bearer lowacr" \
    "Bearer no-relation" \
    "browser-cookie-secret-sentinel" \
    "browser_raw_query_secret_sentinel" \
    "middleware-secret-issuer-sentinel" \
    "spoofed-browser-context-sentinel"; do
    [[ -n "$sentinel" ]] || continue
    if grep -Fq -- "$sentinel" "${LOG_FILES[@]}" 2>/dev/null; then
      echo "browser runtime log disclosed protected request data" >&2
      status=1
    fi
  done
  exit "$status"
}
trap cleanup EXIT

if [[ "$MIDDLEWARE" == "1" ]]; then
  LOG_FILES+=("$BROKER_LOG")
  "$WASMTIME_BIN" serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --addr "127.0.0.1:${BROKER_PORT}" \
    "$ROOT/tests/middleware-artifacts/mock-authn-broker.wasm" \
    >"$BROKER_LOG" 2>&1 &
  BROKER_PID=$!
  for _ in $(seq 1 100); do
    if ! kill -0 "$BROKER_PID" 2>/dev/null; then
      cat "$BROKER_LOG" >&2 || true
      echo "authentication broker exited before readiness" >&2
      exit 1
    fi
    status="$(curl --silent --max-time 1 --request POST --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${BROKER_PORT}/authenticate" || true)"
    [[ "$status" == "401" ]] && break
    sleep 0.05
  done
  [[ "${status:-000}" == "401" ]]
fi
if [[ "$AUTHZ" == "1" ]]; then
  LOG_FILES+=("$PDP_LOG")
  "$WASMTIME_BIN" serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --env=WASI_AUTHZ_PDP_BEARER_TOKEN="$AUTHZ_PDP_TOKEN" \
    --addr "127.0.0.1:${CEDAR_PDP_PORT}" \
    "$ROOT/tests/authz-artifacts/cedar-pdp.wasm" \
    >"$PDP_LOG" 2>&1 &
  PDP_PID=$!
  for _ in $(seq 1 100); do
    if ! kill -0 "$PDP_PID" 2>/dev/null; then
      cat "$PDP_LOG" >&2 || true
      echo "Cedar PDP exited before readiness" >&2
      exit 1
    fi
    status="$(curl --silent --max-time 1 --request POST --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${CEDAR_PDP_PORT}/access/v1/evaluation" || true)"
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
    WASMTIME_ARGS=(
      serve
      -W component-model-async=y
      -S p3=y
      -S cli=y
      -S http=y
    )
    if [[ "$MIDDLEWARE" == "1" ]]; then
      WASMTIME_ARGS+=(
        -S inherit-network=y
        --env=WASI_MIDDLEWARE_CORS_ORIGINS="http://127.0.0.1:$PORT"
        --env=WASI_MIDDLEWARE_CORS_METHODS=GET,HEAD,POST
        --env=WASI_MIDDLEWARE_CORS_HEADERS=content-type,authorization,x-request-id
        --env=WASI_MIDDLEWARE_CORS_ALLOW_CREDENTIALS=false
        --env=WASI_MIDDLEWARE_AUTHN_BROKER_URL=http://127.0.0.1:${BROKER_PORT}/authenticate
        --env=WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS=2000
        --env=WASI_MIDDLEWARE_AUTHN_MODE=optional
        --env=WASI_MIDDLEWARE_SERVICE_ID=leptos-wasi-counter
        --env=WASI_MIDDLEWARE_AUTHN_AUDIENCES=api://leptos-wasi-counter
        --env=WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT="$AUTHN_MAX_IN_FLIGHT"
        --env=WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK=true
      )
    fi
    if [[ "$AUTHZ" == "1" ]]; then
      WASMTIME_ARGS+=(
        --env=WASI_AUTHZ_SERVICE_ID=leptos-wasi-counter
        --env=WASI_AUTHZ_ENDPOINT=http://127.0.0.1:${CEDAR_PDP_PORT}/access/v1/evaluation
        --env=WASI_AUTHZ_TIMEOUT_MS=2000
        --env=WASI_AUTHZ_ALLOW_LOOPBACK_DEV=true
        --env=WASI_AUTHZ_PDP_BEARER_TOKEN="$AUTHZ_PDP_TOKEN"
        --env=WASI_AUTHZ_CEDAR_ENDPOINT=http://127.0.0.1:${CEDAR_PDP_PORT}/access/v1/evaluation
        --env=WASI_AUTHZ_SPICEDB_ENDPOINT="$SPICEDB_PDP_URL"
        --env=WASI_AUTHZ_SPICEDB_PDP_BEARER_TOKEN="$SPICEDB_PDP_TOKEN"
      )
    fi
    WASMTIME_ARGS+=(
      --dir="$PWD/target/site::/site"
      --env=LEPTOS_OUTPUT_NAME=counter
      --env=LEPTOS_SITE_ROOT=/site
      --env=LEPTOS_SITE_PKG_DIR=pkg
      --addr "127.0.0.1:$PORT"
      "$SERVER_COMPONENT"
    )
    "$WASMTIME_BIN" "${WASMTIME_ARGS[@]}" >"$APP_LOG" 2>&1 &
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

SERVER_PID=$!

for _ in $(seq 1 60); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    cat "$APP_LOG" >&2 || true
    echo "browser test application exited before readiness" >&2
    exit 1
  fi
  if curl --fail --silent "http://127.0.0.1:$PORT/" >/dev/null; then
    break
  fi
  sleep 1
done
curl --fail --silent "http://127.0.0.1:$PORT/" >/dev/null

ACTION_URL="http://127.0.0.1:$PORT/api/increment_count"
if [[ "$AUTHZ_FULL_CHAIN_BENCHMARK" == "1" ]]; then
  [[ "$AUTHZ" == "1" ]] || {
    echo "the full-chain benchmark requires AUTHZ=1" >&2
    exit 2
  }
  OHA_BIN="$(resolve_middleware_tool OHA_BIN oha "$(middleware_lock_value oha_version)")"
  BENCHMARK_DIR="$ROOT/target/authz-full-chain-benchmark"
  mkdir -p "$BENCHMARK_DIR"
  NO_COLOR=true "$OHA_BIN" \
    -n 500 -c 100 --http-version 1.1 -t 10s --no-tui \
    --output-format json \
    -m POST \
    -H "authorization: Bearer allow" \
    -H "content-type: application/x-www-form-urlencoded" \
    -d "current=0" \
    "$ACTION_URL" \
    >"$BENCHMARK_DIR/warmup.json"
  NO_COLOR=true "$OHA_BIN" \
    -n "$AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS" \
    -c 100 --http-version 1.1 -t 10s --no-tui \
    --output-format json \
    -m POST \
    -H "authorization: Bearer allow" \
    -H "content-type: application/x-www-form-urlencoded" \
    -d "current=0" \
    "$ACTION_URL" \
    >"$BENCHMARK_DIR/result.json"
  python3 "$ROOT/scripts/check-authz-full-chain-performance.py" \
    "$BENCHMARK_DIR/result.json" \
    "$BENCHMARK_DIR/summary.json"
fi

if [[ "$AUTHZ_FULL_CHAIN_SOAK" == "1" ]]; then
  [[ "$AUTHZ" == "1" ]] || {
    echo "the full-chain soak requires AUTHZ=1" >&2
    exit 2
  }
  mkdir -p "$ROOT/target/authz-full-chain-soak"
  python3 "$ROOT/scripts/load_runtime.py" \
    "$ACTION_URL" \
    --duration "$AUTHZ_FULL_CHAIN_SOAK_DURATION" \
    --concurrency 100 \
    --pid "$SERVER_PID" \
    --method POST \
    --header "authorization: Bearer allow" \
    --header "content-type: application/x-www-form-urlencoded" \
    --body "current=0" \
    | tee "$ROOT/target/authz-full-chain-soak/result.json"
fi

if [[ "$AUTHZ_FULL_CHAIN_BENCHMARK_ONLY" == "1" ]]; then
  exit 0
fi

cd "$ROOT/tests/browser"
if [[ ! -x node_modules/.bin/playwright ]]; then
  npm ci
fi
BASE_URL="http://127.0.0.1:$PORT" MIDDLEWARE="$MIDDLEWARE" AUTHZ="$AUTHZ" npm test

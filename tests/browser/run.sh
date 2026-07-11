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
AUTHENTICATION_MODE="${AUTHENTICATION_MODE:-}"
if [[ -z "$AUTHENTICATION_MODE" ]]; then
  if [[ "$MIDDLEWARE" == "1" ]]; then
    AUTHENTICATION_MODE="portable_component"
  else
    AUTHENTICATION_MODE="none"
  fi
fi
PORTABLE_COMPONENT=0
TRUSTED_INGRESS=0
case "$AUTHENTICATION_MODE" in
  none) ;;
  portable_component) PORTABLE_COMPONENT=1 ;;
  trusted_ingress) TRUSTED_INGRESS=1 ;;
  *) echo "unsupported AUTHENTICATION_MODE: $AUTHENTICATION_MODE" >&2; exit 2 ;;
esac
EDGE_POLICY=$((PORTABLE_COMPONENT || TRUSTED_INGRESS))
AUTHZ_PDP_TOKEN="${AUTHZ_PDP_TOKEN:-leptos-wasi-browser-pdp-token-do-not-log}"
SPICEDB_PDP_URL="${WASI_AUTHZ_TEST_PDP_URL:-}"
SPICEDB_PDP_TOKEN="${WASI_AUTHZ_TEST_PDP_BEARER_TOKEN:-}"
AUTHZ_FULL_CHAIN_BENCHMARK="${AUTHZ_FULL_CHAIN_BENCHMARK:-0}"
AUTHZ_FULL_CHAIN_BENCHMARK_ONLY="${AUTHZ_FULL_CHAIN_BENCHMARK_ONLY:-0}"
AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS:-5000}"
AUTHZ_FULL_CHAIN_SOAK="${AUTHZ_FULL_CHAIN_SOAK:-0}"
AUTHZ_FULL_CHAIN_SOAK_DURATION="${AUTHZ_FULL_CHAIN_SOAK_DURATION:-600}"
AUTHZ_FULL_CHAIN_BENCHMARK_PATH="${AUTHZ_FULL_CHAIN_BENCHMARK_PATH:-/api/increment_count}"
# Keep the normal fixture admission bound explicit, while allowing dedicated
# diagnostics to select another validated per-instance limit.
AUTHN_MAX_IN_FLIGHT="${AUTHN_MAX_IN_FLIGHT:-128}"

if [[ "$AUTHZ" == "1" && "$EDGE_POLICY" != "1" ]]; then
  echo "AUTHZ=1 requires portable component or trusted-ingress authentication" >&2
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
if [[ "$PORTABLE_COMPONENT" == "1" ]]; then
  "$ROOT/scripts/audit-middleware-manifests.py"
  "$ROOT/scripts/sync-middleware-components.sh"
  TERMINAL_COMPONENT="$SERVER_COMPONENT"
  SERVER_COMPONENT="target/wasm32-wasip2/release/counter-middleware.wasm"
  DEPLOYMENT_PROFILE="wasmtime-authn"
  if [[ "$AUTHZ" == "1" ]]; then
    DEPLOYMENT_PROFILE="wasmtime-authn-authz"
    if [[ "${AUTHZ_COARSE_PEP:-0}" == "1" ]]; then
      DEPLOYMENT_PROFILE="wasmtime-authn-authz-coarse"
    fi
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
  if [[ "$PORTABLE_COMPONENT" == "1" ]]; then
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
TERMINAL_PORT="${TERMINAL_PORT:-$(available_port)}"
BROKER_PID=""
PDP_PID=""
SERVER_PID=""
TERMINAL_PID=""
APP_LOG="$ROOT/tests/browser/app.log"
TERMINAL_LOG="$ROOT/tests/browser/terminal.log"
PDP_LOG="$ROOT/tests/browser/cedar-pdp.log"
BROKER_LOG="$ROOT/tests/browser/authn-broker.log"
rm -f "$APP_LOG" "$TERMINAL_LOG" "$PDP_LOG" "$BROKER_LOG"
LOG_FILES=("$APP_LOG")
cleanup() {
  status=$?
  trap - EXIT
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  [[ -n "$TERMINAL_PID" ]] && kill "$TERMINAL_PID" 2>/dev/null || true
  [[ -n "$BROKER_PID" ]] && kill "$BROKER_PID" 2>/dev/null || true
  [[ -n "$PDP_PID" ]] && kill "$PDP_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
  [[ -n "$TERMINAL_PID" ]] && wait "$TERMINAL_PID" 2>/dev/null || true
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

if [[ "$EDGE_POLICY" == "1" ]]; then
  LOG_FILES+=("$BROKER_LOG")
  BROKER_ARGS=(
    serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --addr "127.0.0.1:${BROKER_PORT}"
  )
  if [[ "${MIDDLEWARE_DIAGNOSTICS:-0}" == "1" ]]; then
    BROKER_ARGS+=(--env=WASI_MIDDLEWARE_DIAGNOSTICS=true)
  fi
  BROKER_ARGS+=("$ROOT/tests/middleware-artifacts/mock-authn-broker.wasm")
  "$WASMTIME_BIN" "${BROKER_ARGS[@]}" >"$BROKER_LOG" 2>&1 &
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
if [[ "$AUTHZ" == "1" && "${AUTHZ_COARSE_PEP:-0}" == "1" ]]; then
  LOG_FILES+=("$PDP_LOG")
  PDP_ARGS=(
    serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --env=WASI_AUTHZ_PDP_BEARER_TOKEN="$AUTHZ_PDP_TOKEN" \
    --addr "127.0.0.1:${CEDAR_PDP_PORT}"
  )
  if [[ "${MIDDLEWARE_DIAGNOSTICS:-0}" == "1" ]]; then
    PDP_ARGS+=(--env=WASI_MIDDLEWARE_DIAGNOSTICS=true)
  fi
  PDP_ARGS+=("$ROOT/tests/authz-artifacts/cedar-pdp.wasm")
  "$WASMTIME_BIN" "${PDP_ARGS[@]}" >"$PDP_LOG" 2>&1 &
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
    if [[ "$EDGE_POLICY" == "1" ]]; then
      "$ROOT/scripts/audit-middleware-manifests.py" --wasmtime
    fi
    WASMTIME_ARGS=(
      serve
      -W component-model-async=y
      -S p3=y
      -S cli=y
      -S http=y
    )
    if [[ -n "${WASMTIME_MAX_INSTANCE_REUSE_COUNT:-}" ]]; then
      WASMTIME_ARGS+=(
        --max-instance-reuse-count
        "${WASMTIME_MAX_INSTANCE_REUSE_COUNT}"
      )
    fi
    if [[ -n "${WASMTIME_MAX_INSTANCE_CONCURRENT_REUSE_COUNT:-}" ]]; then
      WASMTIME_ARGS+=(
        --max-instance-concurrent-reuse-count
        "${WASMTIME_MAX_INSTANCE_CONCURRENT_REUSE_COUNT}"
      )
    fi
    if [[ -n "${WASMTIME_IDLE_INSTANCE_TIMEOUT:-}" ]]; then
      WASMTIME_ARGS+=(--idle-instance-timeout "${WASMTIME_IDLE_INSTANCE_TIMEOUT}")
    fi
    if [[ "$PORTABLE_COMPONENT" == "1" ]]; then
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
      if [[ "${MIDDLEWARE_DIAGNOSTICS:-0}" == "1" ]]; then
        WASMTIME_ARGS+=(--env=WASI_MIDDLEWARE_DIAGNOSTICS=true)
      fi
    fi
    if [[ "$AUTHZ" == "1" ]]; then
      if [[ "$TRUSTED_INGRESS" == "1" ]]; then
        WASMTIME_ARGS+=(-S inherit-network=y)
      fi
      WASMTIME_ARGS+=(
        --env=WASI_AUTHZ_SPICEDB_ENDPOINT="$SPICEDB_PDP_URL"
        --env=WASI_AUTHZ_SPICEDB_PDP_BEARER_TOKEN="$SPICEDB_PDP_TOKEN"
      )
      if [[ "${AUTHZ_COARSE_PEP:-0}" == "1" ]]; then
        WASMTIME_ARGS+=(
          --env=WASI_AUTHZ_SERVICE_ID=leptos-wasi-counter
          --env=WASI_AUTHZ_ENDPOINT=http://127.0.0.1:${CEDAR_PDP_PORT}/access/v1/evaluation
          --env=WASI_AUTHZ_TIMEOUT_MS=2000
          --env=WASI_AUTHZ_ALLOW_LOOPBACK_DEV=true
          --env=WASI_AUTHZ_PDP_BEARER_TOKEN="$AUTHZ_PDP_TOKEN"
        )
      fi
    fi
    TARGET_PORT="$PORT"
    if [[ "$TRUSTED_INGRESS" == "1" ]]; then
      TARGET_PORT="$TERMINAL_PORT"
    fi
    WASMTIME_ARGS+=(
      --dir="$PWD/target/site::/site"
      --env=LEPTOS_OUTPUT_NAME=counter
      --env=LEPTOS_SITE_ROOT=/site
      --env=LEPTOS_SITE_PKG_DIR=pkg
      --addr "127.0.0.1:$TARGET_PORT"
      "$SERVER_COMPONENT"
    )
    if [[ "$TRUSTED_INGRESS" == "1" ]]; then
      LOG_FILES+=("$TERMINAL_LOG")
      "$WASMTIME_BIN" "${WASMTIME_ARGS[@]}" >"$TERMINAL_LOG" 2>&1 &
      TERMINAL_PID=$!
      for _ in $(seq 1 100); do
        if ! kill -0 "$TERMINAL_PID" 2>/dev/null; then
          cat "$TERMINAL_LOG" >&2 || true
          echo "trusted terminal exited before readiness" >&2
          exit 1
        fi
        status="$(curl --silent --max-time 1 --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:$TERMINAL_PORT/" || true)"
        [[ "$status" == "503" ]] && break
        sleep 0.05
      done
      [[ "${status:-000}" == "503" ]] || {
        echo "direct terminal did not fail closed without ingress context" >&2
        exit 1
      }
      cargo build --locked --release \
        --manifest-path "$ROOT/tests/trusted-ingress/Cargo.toml"
      TRUSTED_INGRESS_LISTEN_ADDR="127.0.0.1:$PORT" \
      TRUSTED_INGRESS_TERMINAL_ORIGIN="http://127.0.0.1:$TERMINAL_PORT" \
      TRUSTED_INGRESS_AUTHN_BROKER_URL="http://127.0.0.1:$BROKER_PORT/authenticate" \
      TRUSTED_INGRESS_SERVICE_ID=leptos-wasi-counter \
      TRUSTED_INGRESS_AUDIENCES=api://leptos-wasi-counter \
      TRUSTED_INGRESS_CORS_ORIGIN="http://127.0.0.1:$PORT" \
      TRUSTED_INGRESS_POLICY_ENABLED="${TRUSTED_INGRESS_POLICY_ENABLED:-true}" \
        "$ROOT/tests/trusted-ingress/target/release/leptos-wasi-trusted-ingress" \
        >"$APP_LOG" 2>&1 &
    else
      "$WASMTIME_BIN" "${WASMTIME_ARGS[@]}" >"$APP_LOG" 2>&1 &
    fi
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

ACTION_URL="http://127.0.0.1:$PORT$AUTHZ_FULL_CHAIN_BENCHMARK_PATH"
if [[ "$AUTHZ_FULL_CHAIN_BENCHMARK" == "1" ]]; then
  [[ "$AUTHZ" == "1" ]] || {
    echo "the full-chain benchmark requires AUTHZ=1" >&2
    exit 2
  }
  OHA_BIN="$(resolve_middleware_tool OHA_BIN oha "$(middleware_lock_value oha_version)")"
  BENCHMARK_DIR="${AUTHZ_FULL_CHAIN_BENCHMARK_DIR:-$ROOT/target/authz-full-chain-benchmark}"
  BENCHMARK_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS:-5000}"
  BENCHMARK_WARMUP_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_WARMUP_REQUESTS:-500}"
  BENCHMARK_CONCURRENCY="${AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY:-100}"
  BENCHMARK_RATE="${AUTHZ_FULL_CHAIN_BENCHMARK_RATE:-}"
  BENCHMARK_METHOD="${AUTHZ_FULL_CHAIN_BENCHMARK_METHOD:-POST}"
  mkdir -p "$BENCHMARK_DIR"
  OHA_ARGS=(
    --http-version 1.1
    -t 10s
    --no-tui
    --output-format json
    -m "$BENCHMARK_METHOD"
    "$ACTION_URL"
  )
  if [[ "$BENCHMARK_METHOD" == "POST" ]]; then
    OHA_ARGS=(-H "authorization: Bearer allow" -H "content-type: application/x-www-form-urlencoded" -d "current=0" "${OHA_ARGS[@]}")
  fi
  if [[ -n "$BENCHMARK_RATE" ]]; then
    OHA_ARGS=(-q "$BENCHMARK_RATE" "${OHA_ARGS[@]}")
  fi
  NO_COLOR=true "$OHA_BIN" -n "$BENCHMARK_WARMUP_REQUESTS" \
    -c "$BENCHMARK_CONCURRENCY" "${OHA_ARGS[@]}" \
    >"$BENCHMARK_DIR/warmup.json"
  NO_COLOR=true "$OHA_BIN" -n "$BENCHMARK_REQUESTS" \
    -c "$BENCHMARK_CONCURRENCY" "${OHA_ARGS[@]}" \
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
    --concurrency "${AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY:-100}" \
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
BASE_URL="http://127.0.0.1:$PORT" MIDDLEWARE="$EDGE_POLICY" AUTHZ="$AUTHZ" npm test

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
SPICEDB_URL="${WASI_AUTHZ_TEST_SPICEDB_URL:-}"
SPICEDB_TOKEN="${WASI_AUTHZ_TEST_SPICEDB_TOKEN:-}"
SPICEDB_POLICY_REVISION="${WASI_AUTHZ_TEST_SPICEDB_POLICY_REVISION:-}"
SPICEDB_MODEL_VERSION="${WASI_AUTHZ_TEST_SPICEDB_MODEL_VERSION:-}"
AUTHZ_FULL_CHAIN_BENCHMARK="${AUTHZ_FULL_CHAIN_BENCHMARK:-0}"
AUTHZ_FULL_CHAIN_BENCHMARK_ONLY="${AUTHZ_FULL_CHAIN_BENCHMARK_ONLY:-0}"
AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS:-5000}"
AUTHZ_FULL_CHAIN_SOAK="${AUTHZ_FULL_CHAIN_SOAK:-0}"
AUTHZ_FULL_CHAIN_SOAK_DURATION="${AUTHZ_FULL_CHAIN_SOAK_DURATION:-600}"
AUTHZ_FULL_CHAIN_BENCHMARK_PATH="${AUTHZ_FULL_CHAIN_BENCHMARK_PATH:-/api/increment_count}"
AUTHZ_FULL_CHAIN_SCENARIO="${AUTHZ_FULL_CHAIN_SCENARIO:-$ROOT/tests/trusted-ingress/scenarios/hybrid-direct.toml}"
# Keep the normal fixture admission bound explicit, while allowing dedicated
# diagnostics to select another validated per-instance limit.
AUTHN_MAX_IN_FLIGHT="${AUTHN_MAX_IN_FLIGHT:-128}"

if [[ "$AUTHZ" == "1" && "$EDGE_POLICY" != "1" ]]; then
  echo "AUTHZ=1 requires portable component or trusted-ingress authentication" >&2
  exit 2
fi
if [[ "$AUTHZ" == "1" && ( -z "$SPICEDB_URL" || -z "$SPICEDB_TOKEN" || -z "$SPICEDB_POLICY_REVISION" || -z "$SPICEDB_MODEL_VERSION" ) ]]; then
  echo "AUTHZ=1 must run through scripts/run-authz-browser.sh" >&2
  exit 2
fi
if [[ "$AUTHZ" == "1" ]]; then
  [[ "${AUTHZ_TEST_ONLY:-}" == "1" ]] || {
    echo "AUTHZ=1 browser runs are test-only; use scripts/run-authz-browser.sh" >&2
    exit 2
  }
  case "$SPICEDB_URL" in
    http://127.0.0.1:*|http://localhost:*) ;;
    *)
      echo "AUTHZ=1 browser runs require a loopback SpiceDB endpoint" >&2
      exit 2
      ;;
  esac
  [[ "$AUTHZ_PDP_TOKEN" == "leptos-wasi-browser-pdp-token-do-not-log" ]] || {
    echo "AUTHZ=1 browser runs refuse a non-fixture Cedar PDP credential" >&2
    exit 2
  }
  [[ "$SPICEDB_TOKEN" == spicedb-component-live-secret-* ]] || {
    echo "AUTHZ=1 browser runs refuse a non-fixture SpiceDB credential" >&2
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

if [[ "$HOST" == "spin" && "$TRUSTED_INGRESS" != "1" ]]; then
  if [[ "$PORTABLE_COMPONENT" == "1" ]]; then
    "$ROOT/scripts/audit-middleware-manifests.py" --spin-main
    manifest="$PWD/spin.middleware-composed.toml"
  else
    manifest="$PWD/spin.toml"
  fi
  SPIN_MANIFEST="$manifest" \
  PORT="$PORT" \
  MIDDLEWARE="$MIDDLEWARE" \
  AUTHZ="$AUTHZ" \
    exec "$ROOT/tests/browser/run-spin-main.sh"
fi

WASMTIME_BIN="$(resolve_middleware_tool WASMTIME_BIN wasmtime "$(middleware_lock_value wasmtime_version)")"
BROKER_PORT="${BROKER_PORT:-$(available_port)}"
CEDAR_PDP_PORT="${CEDAR_PDP_PORT:-$(available_port)}"
TERMINAL_PORT="${TERMINAL_PORT:-$(available_port)}"
TERMINAL_REPLICAS="${TERMINAL_REPLICAS:-1}"
[[ "$TERMINAL_REPLICAS" =~ ^[1-4]$ ]] || {
  echo "TERMINAL_REPLICAS must be between 1 and 4" >&2
  exit 2
}
DIAGNOSTICS_PORT="${DIAGNOSTICS_PORT:-$(available_port)}"
PROCESS_FILE="${TRUSTED_TOPOLOGY_PROCESS_FILE:-$ROOT/target/trusted-topology/processes.json}"
BROKER_PID=""
PDP_PID=""
SERVER_PID=""
TERMINAL_PIDS=()
TERMINAL_PORTS=()
TERMINAL_LOGS=()
SPIN_RUNTIME_VERSION=""
APP_LOG="$ROOT/tests/browser/app.log"
PDP_LOG="$ROOT/tests/browser/cedar-pdp.log"
BROKER_LOG="$ROOT/tests/browser/authn-broker.log"
rm -f "$APP_LOG" "$ROOT/tests/browser/terminal-"*.log "$PDP_LOG" "$BROKER_LOG"
LOG_FILES=("$APP_LOG")
cleanup() {
  status=$?
  trap - EXIT
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  for pid in ${TERMINAL_PIDS[@]+"${TERMINAL_PIDS[@]}"}; do kill "$pid" 2>/dev/null || true; done
  [[ -n "$BROKER_PID" ]] && kill "$BROKER_PID" 2>/dev/null || true
  [[ -n "$PDP_PID" ]] && kill "$PDP_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
  for pid in ${TERMINAL_PIDS[@]+"${TERMINAL_PIDS[@]}"}; do wait "$pid" 2>/dev/null || true; done
  [[ -n "$BROKER_PID" ]] && wait "$BROKER_PID" 2>/dev/null || true
  [[ -n "$PDP_PID" ]] && wait "$PDP_PID" 2>/dev/null || true
  for sentinel in \
    "$AUTHZ_PDP_TOKEN" \
    "$SPICEDB_PDP_TOKEN" \
    "$SPICEDB_TOKEN" \
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
  if [[ -n "${BROKER_MAX_INSTANCE_REUSE_COUNT:-}" ]]; then
    BROKER_ARGS+=(--max-instance-reuse-count "$BROKER_MAX_INSTANCE_REUSE_COUNT")
  fi
  if [[ -n "${BROKER_MAX_INSTANCE_CONCURRENT_REUSE_COUNT:-}" ]]; then
    BROKER_ARGS+=(--max-instance-concurrent-reuse-count "$BROKER_MAX_INSTANCE_CONCURRENT_REUSE_COUNT")
  fi
  if [[ -n "${BROKER_IDLE_INSTANCE_TIMEOUT:-}" ]]; then
    BROKER_ARGS+=(--idle-instance-timeout "$BROKER_IDLE_INSTANCE_TIMEOUT")
  fi
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
    TERMINAL_REUSE_COUNT="${TERMINAL_MAX_INSTANCE_REUSE_COUNT:-${WASMTIME_MAX_INSTANCE_REUSE_COUNT:-}}"
    TERMINAL_CONCURRENT_REUSE_COUNT="${TERMINAL_MAX_INSTANCE_CONCURRENT_REUSE_COUNT:-${WASMTIME_MAX_INSTANCE_CONCURRENT_REUSE_COUNT:-}}"
    TERMINAL_IDLE_TIMEOUT="${TERMINAL_IDLE_INSTANCE_TIMEOUT:-${WASMTIME_IDLE_INSTANCE_TIMEOUT:-}}"
    if [[ -n "$TERMINAL_REUSE_COUNT" ]]; then
      WASMTIME_ARGS+=(
        --max-instance-reuse-count
        "$TERMINAL_REUSE_COUNT"
      )
    fi
    if [[ -n "$TERMINAL_CONCURRENT_REUSE_COUNT" ]]; then
      WASMTIME_ARGS+=(
        --max-instance-concurrent-reuse-count
        "$TERMINAL_CONCURRENT_REUSE_COUNT"
      )
    fi
    if [[ -n "$TERMINAL_IDLE_TIMEOUT" ]]; then
      WASMTIME_ARGS+=(--idle-instance-timeout "$TERMINAL_IDLE_TIMEOUT")
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
        --env=WASI_AUTHZ_SPICEDB_ENDPOINT="$SPICEDB_URL"
        --env=WASI_AUTHZ_SPICEDB_BEARER_TOKEN="$SPICEDB_TOKEN"
        --env=WASI_AUTHZ_SPICEDB_POLICY_REVISION="$SPICEDB_POLICY_REVISION"
        --env=WASI_AUTHZ_SPICEDB_MODEL_VERSION="$SPICEDB_MODEL_VERSION"
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
    WASMTIME_ARGS+=(
      --dir="$PWD/target/site::/site"
      --env=LEPTOS_OUTPUT_NAME=counter
      --env=LEPTOS_SITE_ROOT=/site
      --env=LEPTOS_SITE_PKG_DIR=pkg
    )
    if [[ "$TRUSTED_INGRESS" == "1" ]]; then
      for replica in $(seq 1 "$TERMINAL_REPLICAS"); do
        if [[ "$replica" == "1" ]]; then
          replica_port="$TERMINAL_PORT"
        else
          replica_port="$(available_port)"
        fi
        replica_log="$ROOT/tests/browser/terminal-${replica}.log"
        TERMINAL_PORTS+=("$replica_port")
        TERMINAL_LOGS+=("$replica_log")
        LOG_FILES+=("$replica_log")
        "$WASMTIME_BIN" "${WASMTIME_ARGS[@]}" \
          --addr "127.0.0.1:$replica_port" "$SERVER_COMPONENT" \
          >"$replica_log" 2>&1 &
        TERMINAL_PIDS+=("$!")
      done
      for replica_index in "${!TERMINAL_PIDS[@]}"; do
        terminal_pid="${TERMINAL_PIDS[$replica_index]}"
        replica_port="${TERMINAL_PORTS[$replica_index]}"
        replica_log="${TERMINAL_LOGS[$replica_index]}"
        status=000
        for _ in $(seq 1 100); do
          if ! kill -0 "$terminal_pid" 2>/dev/null; then
            rtk cat "$replica_log" >&2 || true
            echo "trusted terminal replica $((replica_index + 1)) exited before readiness" >&2
            exit 1
          fi
          status="$(curl --silent --max-time 1 --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:$replica_port/" || true)"
          [[ "$status" == "503" ]] && break
          sleep 0.05
        done
        [[ "$status" == "503" ]] || {
          echo "direct terminal replica $((replica_index + 1)) did not fail closed" >&2
          exit 1
        }
      done
      terminal_origins=""
      for replica_port in "${TERMINAL_PORTS[@]}"; do
        terminal_origins="${terminal_origins:+${terminal_origins},}http://127.0.0.1:${replica_port}"
      done
      rtk cargo build --locked --release \
        --manifest-path "$ROOT/tests/trusted-ingress/Cargo.toml"
      TRUSTED_INGRESS_LISTEN_ADDR="127.0.0.1:$PORT" \
      TRUSTED_INGRESS_DIAGNOSTICS_ADDR="127.0.0.1:$DIAGNOSTICS_PORT" \
      TRUSTED_INGRESS_TERMINAL_ORIGINS="$terminal_origins" \
      TRUSTED_INGRESS_AUTHN_BROKER_URL="http://127.0.0.1:$BROKER_PORT/authenticate" \
      TRUSTED_INGRESS_SERVICE_ID=leptos-wasi-counter \
      TRUSTED_INGRESS_AUDIENCES=api://leptos-wasi-counter \
      TRUSTED_INGRESS_CORS_ORIGIN="http://127.0.0.1:$PORT" \
      TRUSTED_INGRESS_ROUTE_POLICY="$ROOT/tests/trusted-ingress/routes.toml" \
      TRUSTED_INGRESS_PROFILE="${TRUSTED_INGRESS_PROFILE:-edge-authenticated}" \
      TRUSTED_INGRESS_POLICY_ENABLED="${TRUSTED_INGRESS_POLICY_ENABLED:-true}" \
        "$ROOT/tests/trusted-ingress/target/release/leptos-wasi-trusted-ingress" \
        >"$APP_LOG" 2>&1 &
    else
      "$WASMTIME_BIN" "${WASMTIME_ARGS[@]}" \
        --addr "127.0.0.1:$PORT" "$SERVER_COMPONENT" >"$APP_LOG" 2>&1 &
    fi
    ;;
  spin)
    [[ "$TRUSTED_INGRESS" == "1" && "$AUTHZ" == "1" ]] || {
      echo "Spin main is supported here only as a trusted-ingress authorization terminal" >&2
      exit 2
    }
    SPIN_BIN="$(resolve_spin_main_tool)"
    SPIN_RUNTIME_VERSION="$("$SPIN_BIN" --version 2>&1)"
    SPIN_TERMINAL_MANIFEST="$PWD/target/spin-trusted-ingress.toml"
    SPIN_SERVER_COMPONENT="$PWD/target/spin-trusted-terminal.wasm"
    mkdir -p "$(dirname "$SPIN_TERMINAL_MANIFEST")"
    install -m 0644 "$SERVER_COMPONENT" "$SPIN_SERVER_COMPONENT"
    SERVER_COMPONENT="$SPIN_SERVER_COMPONENT" \
    SITE_ROOT="$PWD/target/site" \
    SPICEDB_URL="$SPICEDB_URL" \
    SPICEDB_TOKEN="$SPICEDB_TOKEN" \
    SPICEDB_POLICY_REVISION="$SPICEDB_POLICY_REVISION" \
    SPICEDB_MODEL_VERSION="$SPICEDB_MODEL_VERSION" \
    SPIN_TERMINAL_MANIFEST="$SPIN_TERMINAL_MANIFEST" \
      python3 - <<'PY'
import json
import os
from urllib.parse import urlsplit

endpoint = urlsplit(os.environ["SPICEDB_URL"])
origin = f"{endpoint.scheme}://{endpoint.netloc}"
quote = json.dumps
document = f'''spin_manifest_version = 2

[application]
name = "leptos-wasi-trusted-terminal"
version = "0.4.2-alpha.3"

[[trigger.http]]
route = "/..."
component = "terminal"
executor = {{ type = "http" }}

[component.terminal]
source = {quote(os.environ["SERVER_COMPONENT"])}
allowed_outbound_hosts = [{quote(origin)}]
files = [{{ source = {quote(os.environ["SITE_ROOT"])}, destination = "/site" }}]

[component.terminal.environment]
LEPTOS_OUTPUT_NAME = "counter"
LEPTOS_SITE_ROOT = "/site"
LEPTOS_SITE_PKG_DIR = "pkg"
WASI_AUTHZ_SPICEDB_ENDPOINT = {quote(os.environ["SPICEDB_URL"])}
WASI_AUTHZ_SPICEDB_BEARER_TOKEN = {quote(os.environ["SPICEDB_TOKEN"])}
WASI_AUTHZ_SPICEDB_POLICY_REVISION = {quote(os.environ["SPICEDB_POLICY_REVISION"])}
WASI_AUTHZ_SPICEDB_MODEL_VERSION = {quote(os.environ["SPICEDB_MODEL_VERSION"])}
'''
with open(os.environ["SPIN_TERMINAL_MANIFEST"], "w", encoding="utf-8") as output:
    output.write(document)
PY
    for replica in $(seq 1 "$TERMINAL_REPLICAS"); do
      if [[ "$replica" == "1" ]]; then
        replica_port="$TERMINAL_PORT"
      else
        replica_port="$(available_port)"
      fi
      replica_log="$ROOT/tests/browser/terminal-${replica}.log"
      TERMINAL_PORTS+=("$replica_port")
      TERMINAL_LOGS+=("$replica_log")
      LOG_FILES+=("$replica_log")
      "$SPIN_BIN" up --file "$SPIN_TERMINAL_MANIFEST" \
        --listen "127.0.0.1:$replica_port" >"$replica_log" 2>&1 &
      TERMINAL_PIDS+=("$!")
    done
    for replica_index in "${!TERMINAL_PIDS[@]}"; do
      terminal_pid="${TERMINAL_PIDS[$replica_index]}"
      replica_port="${TERMINAL_PORTS[$replica_index]}"
      replica_log="${TERMINAL_LOGS[$replica_index]}"
      status=000
      for _ in $(seq 1 200); do
        if ! kill -0 "$terminal_pid" 2>/dev/null; then
          cat "$replica_log" >&2 || true
          echo "Spin terminal replica $((replica_index + 1)) exited before readiness" >&2
          exit 1
        fi
        status="$(curl --silent --max-time 1 --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:$replica_port/" || true)"
        [[ "$status" == "503" ]] && break
        sleep 0.05
      done
      [[ "$status" == "503" ]] || {
        echo "direct Spin terminal replica $((replica_index + 1)) did not fail closed" >&2
        exit 1
      }
    done
    terminal_origins=""
    for replica_port in "${TERMINAL_PORTS[@]}"; do
      terminal_origins="${terminal_origins:+${terminal_origins},}http://127.0.0.1:${replica_port}"
    done
    rtk cargo build --locked --release \
      --manifest-path "$ROOT/tests/trusted-ingress/Cargo.toml"
    TRUSTED_INGRESS_LISTEN_ADDR="127.0.0.1:$PORT" \
    TRUSTED_INGRESS_DIAGNOSTICS_ADDR="127.0.0.1:$DIAGNOSTICS_PORT" \
    TRUSTED_INGRESS_TERMINAL_ORIGINS="$terminal_origins" \
    TRUSTED_INGRESS_AUTHN_BROKER_URL="http://127.0.0.1:$BROKER_PORT/authenticate" \
    TRUSTED_INGRESS_SERVICE_ID=leptos-wasi-counter \
    TRUSTED_INGRESS_AUDIENCES=api://leptos-wasi-counter \
    TRUSTED_INGRESS_CORS_ORIGIN="http://127.0.0.1:$PORT" \
    TRUSTED_INGRESS_ROUTE_POLICY="$ROOT/tests/trusted-ingress/routes.toml" \
    TRUSTED_INGRESS_PROFILE="${TRUSTED_INGRESS_PROFILE:-edge-authenticated}" \
    TRUSTED_INGRESS_POLICY_ENABLED="${TRUSTED_INGRESS_POLICY_ENABLED:-true}" \
      "$ROOT/tests/trusted-ingress/target/release/leptos-wasi-trusted-ingress" \
      >"$APP_LOG" 2>&1 &
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

SERVER_PID=$!

if [[ "$TRUSTED_INGRESS" == "1" ]]; then
  mkdir -p "$(dirname "$PROCESS_FILE")"
  PROCESS_FILE="$PROCESS_FILE" \
  INGRESS_PID="$SERVER_PID" \
  INGRESS_PORT="$PORT" \
  DIAGNOSTICS_PORT="$DIAGNOSTICS_PORT" \
  BROKER_PID="$BROKER_PID" \
  BROKER_PORT="$BROKER_PORT" \
  TERMINAL_PIDS="${TERMINAL_PIDS[*]}" \
  TERMINAL_PORTS="${TERMINAL_PORTS[*]}" \
  SPICEDB_PID="${WASI_AUTHZ_TEST_SPICEDB_PID:-}" \
  AUTHZEN_PDP_PID="${WASI_AUTHZ_TEST_AUTHZEN_PDP_PID:-}" \
  TERMINAL_REPLICAS="$TERMINAL_REPLICAS" \
  INGRESS_PROFILE="${TRUSTED_INGRESS_PROFILE:-edge-authenticated}" \
  TERMINAL_RUNTIME="$HOST" \
  SPIN_RUNTIME_VERSION="$SPIN_RUNTIME_VERSION" \
  WASMTIME_VERSION="$(middleware_lock_value wasmtime_version)" \
  SPICEDB_VERSION="${WASI_AUTHZ_TEST_SPICEDB_VERSION:-unknown}" \
    python3 -c '
import json, os
processes = [
    {"name": "ingress", "pid": int(os.environ["INGRESS_PID"]), "port": int(os.environ["INGRESS_PORT"])},
    {"name": "authentication-broker", "pid": int(os.environ["BROKER_PID"]), "port": int(os.environ["BROKER_PORT"])},
]
for index, (pid, port) in enumerate(zip(os.environ["TERMINAL_PIDS"].split(), os.environ["TERMINAL_PORTS"].split()), 1):
    processes.append({"name": f"terminal-{index}", "pid": int(pid), "port": int(port)})
for name, key in (("spicedb", "SPICEDB_PID"), ("authzen-pdp", "AUTHZEN_PDP_PID")):
    if os.environ.get(key):
        processes.append({"name": name, "pid": int(os.environ[key])})
with open(os.environ["PROCESS_FILE"], "w", encoding="utf-8") as output:
    json.dump({
        "schema": 1,
        "diagnostics_url": "http://127.0.0.1:" + os.environ["DIAGNOSTICS_PORT"] + "/",
        "versions": {
            "wasmtime": os.environ["WASMTIME_VERSION"],
            "spin": os.environ["SPIN_RUNTIME_VERSION"] or None,
            "spicedb": os.environ["SPICEDB_VERSION"],
        },
        "configuration": {
            "terminal_runtime": os.environ["TERMINAL_RUNTIME"],
            "terminal_replicas": int(os.environ["TERMINAL_REPLICAS"]),
            "profile": os.environ["INGRESS_PROFILE"],
            "route_policy": "tests/trusted-ingress/routes.toml",
        },
        "processes": processes,
    }, output, indent=2)
'
fi

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
  BENCHMARK_DIR="${AUTHZ_FULL_CHAIN_BENCHMARK_DIR:-$ROOT/target/authz-full-chain-benchmark}"
  BENCHMARK_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS:-5000}"
  BENCHMARK_WARMUP_REQUESTS="${AUTHZ_FULL_CHAIN_BENCHMARK_WARMUP_REQUESTS:-500}"
  BENCHMARK_CONCURRENCY="${AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY:-100}"
  BENCHMARK_MODE="${AUTHZ_FULL_CHAIN_BENCHMARK_MODE:-fixed}"
  BENCHMARK_DURATION="${AUTHZ_FULL_CHAIN_BENCHMARK_DURATION:-60}"
  mkdir -p "$BENCHMARK_DIR"
  rtk cargo build --locked --release \
    --manifest-path "$ROOT/tests/trusted-load/Cargo.toml"
  benchmark_status=0
  benchmark_load_args=(
    --base-url "http://127.0.0.1:$PORT"
    --scenario "$AUTHZ_FULL_CHAIN_SCENARIO"
    --mode "$BENCHMARK_MODE"
    --warmup-requests "$BENCHMARK_WARMUP_REQUESTS"
    --seed "${AUTHZ_FULL_CHAIN_BENCHMARK_SEED:-0}"
    --concurrency "$BENCHMARK_CONCURRENCY"
    --process-file "$PROCESS_FILE"
    --output "$BENCHMARK_DIR/result.json"
  )
  case "$BENCHMARK_MODE" in
    fixed)
      benchmark_load_args+=(--requests "$BENCHMARK_REQUESTS")
      ;;
    open-loop)
      [[ -n "${AUTHZ_FULL_CHAIN_BENCHMARK_RATE:-}" ]] || {
        echo "open-loop benchmark requires AUTHZ_FULL_CHAIN_BENCHMARK_RATE" >&2
        exit 2
      }
      benchmark_load_args+=(
        --duration "$BENCHMARK_DURATION"
        --rate "$AUTHZ_FULL_CHAIN_BENCHMARK_RATE"
      )
      ;;
    *)
      echo "AUTHZ_FULL_CHAIN_BENCHMARK_MODE must be fixed or open-loop" >&2
      exit 2
      ;;
  esac
  TRUSTED_LOAD_CREDENTIAL_ALLOW="Bearer allow" \
    "$ROOT/tests/trusted-load/target/release/trusted-load" \
    "${benchmark_load_args[@]}" || benchmark_status=$?
  "$ROOT/scripts/validate-trusted-load-report.py" \
    "$BENCHMARK_DIR/result.json"
  curl --fail --silent "http://127.0.0.1:$DIAGNOSTICS_PORT/" \
    --output "$BENCHMARK_DIR/diagnostics.json"
  if [[ "$benchmark_status" -ne 0 ]]; then
    exit "$benchmark_status"
  fi
fi

if [[ "$AUTHZ_FULL_CHAIN_SOAK" == "1" ]]; then
  [[ "$AUTHZ" == "1" ]] || {
    echo "the full-chain soak requires AUTHZ=1" >&2
    exit 2
  }
  SOAK_DIR="${AUTHZ_FULL_CHAIN_SOAK_DIR:-$ROOT/target/authz-full-chain-soak}"
  mkdir -p "$SOAK_DIR"
  rtk cargo build --locked --release \
    --manifest-path "$ROOT/tests/trusted-load/Cargo.toml"
  soak_status=0
  TRUSTED_LOAD_CREDENTIAL_ALLOW="Bearer allow" \
    "$ROOT/tests/trusted-load/target/release/trusted-load" \
    --base-url "http://127.0.0.1:$PORT" \
    --scenario "$ROOT/tests/trusted-ingress/scenarios/mixed.toml" \
    --mode closed-loop \
    --duration "$AUTHZ_FULL_CHAIN_SOAK_DURATION" \
    --warmup-requests "${AUTHZ_FULL_CHAIN_SOAK_WARMUP_REQUESTS:-500}" \
    --seed "${AUTHZ_FULL_CHAIN_BENCHMARK_SEED:-0}" \
    --concurrency "${AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY:-100}" \
    --process-file "$PROCESS_FILE" \
    --output "$SOAK_DIR/result.json" || soak_status=$?
  "$ROOT/scripts/validate-trusted-load-report.py" \
    "$SOAK_DIR/result.json"
  curl --fail --silent "http://127.0.0.1:$DIAGNOSTICS_PORT/" \
    --output "$SOAK_DIR/diagnostics.json"
  if [[ "$soak_status" -ne 0 ]]; then
    exit "$soak_status"
  fi
fi

if [[ "$AUTHZ_FULL_CHAIN_BENCHMARK_ONLY" == "1" ]]; then
  exit 0
fi

cd "$ROOT/tests/browser"
if [[ ! -x node_modules/.bin/playwright ]]; then
  npm ci
fi
BASE_URL="http://127.0.0.1:$PORT" MIDDLEWARE="$EDGE_POLICY" AUTHZ="$AUTHZ" npm test

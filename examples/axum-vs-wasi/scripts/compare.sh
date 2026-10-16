#!/usr/bin/env bash
# Build both servers, start them, warm, and print a side-by-side table.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WASI_PORT="${WASI_PORT:-3000}"
AXUM_PORT="${AXUM_PORT:-3001}"
DURATION="${DURATION:-10}"
CONCURRENCY="${CONCURRENCY:-20}"
WARMUP="${WARMUP:-25}"
REUSE_COUNT="${WASMTIME_MAX_INSTANCE_REUSE_COUNT:-${LEPTOS_WASI_MAX_INSTANCE_REUSE:-128}}"
WASI_URL="${WASI_URL:-http://127.0.0.1:${WASI_PORT}}"
AXUM_URL="${AXUM_URL:-http://127.0.0.1:${AXUM_PORT}}"
SKIP_BUILD="${SKIP_BUILD:-0}"
SKIP_START="${SKIP_START:-0}"

WASI_WASM="$ROOT/target/wasm32-wasip2/release/compare_wasi.wasm"
AXUM_BIN="$ROOT/target/release/compare-axum"

if [[ "$SKIP_BUILD" != "1" ]]; then
  echo "building WASI Preview 2 component..."
  cargo build --locked -p compare-wasi --target wasm32-wasip2 --release
  echo "building native axum server..."
  cargo build --locked -p compare-axum --release
fi

if [[ ! -f "$WASI_WASM" ]]; then
  echo "missing $WASI_WASM; build the WASI side first" >&2
  exit 1
fi
if [[ ! -x "$AXUM_BIN" && ! -f "$AXUM_BIN" ]]; then
  echo "missing $AXUM_BIN; build the axum side first" >&2
  exit 1
fi

wait_for() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 60); do
    if curl --fail --silent --max-time 2 "$url/api/get_test" >/dev/null; then
      echo "$label ready at $url"
      return 0
    fi
    sleep 1
  done
  echo "$label did not become ready at $url" >&2
  return 1
}

SERVER_PIDS=()
cleanup() {
  local pid
  for pid in "${SERVER_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
}

if [[ "$SKIP_START" != "1" ]]; then
  trap cleanup EXIT INT TERM

  echo "starting WASI (Preview 2, reuse $REUSE_COUNT) on $WASI_URL"
  wasmtime serve \
    -S cli=y \
    --max-instance-reuse-count "$REUSE_COUNT" \
    --dir "$ROOT/public::/static" \
    --addr "127.0.0.1:${WASI_PORT}" \
    "$WASI_WASM" \
    >/dev/null 2>&1 &
  SERVER_PIDS+=("$!")

  echo "starting axum on $AXUM_URL"
  LEPTOS_SITE_ADDR="127.0.0.1:${AXUM_PORT}" "$AXUM_BIN" >/dev/null 2>&1 &
  SERVER_PIDS+=("$!")

  wait_for "$WASI_URL" "wasi"
  wait_for "$AXUM_URL" "axum"
fi

python3 "$ROOT/scripts/compare.py" \
  --wasi-url "$WASI_URL" \
  --axum-url "$AXUM_URL" \
  --duration "$DURATION" \
  --concurrency "$CONCURRENCY" \
  --warmup "$WARMUP" \
  --reuse "$REUSE_COUNT"

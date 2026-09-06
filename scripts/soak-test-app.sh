#!/usr/bin/env bash
set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "${PROJECT_ROOT:-$TOOL_ROOT}" && pwd)"
HOST="${HOST:-wasmtime}"
PREVIEW="${PREVIEW:-p3}"
PORT="${PORT:-3200}"
DURATION="${DURATION:-600}"
CONCURRENCY="${CONCURRENCY:-100}"
OUTPUT="${OUTPUT:-}"
CHECK_POLLABLES="${CHECK_POLLABLES:-auto}"
if [[ -n "$OUTPUT" && "$OUTPUT" != /* ]]; then
  OUTPUT="$PWD/$OUTPUT"
fi

case "$CHECK_POLLABLES" in
  auto)
    if [[ "$ROOT" == "$TOOL_ROOT" ]]; then
      CHECK_POLLABLES=1
    else
      CHECK_POLLABLES=0
    fi
    ;;
  0 | 1) ;;
  *)
    echo "CHECK_POLLABLES must be auto, 0, or 1" >&2
    exit 2
    ;;
esac

case "$PREVIEW" in
  p2)
    FEATURES="wasip2,islands-router"
    WASM_NAME="test-app-p2.wasm"
    SPIN_MANIFEST="tests/spin.toml"
    ;;
  p3)
    FEATURES="wasip3,islands-router"
    WASM_NAME="test-app-p3.wasm"
    SPIN_MANIFEST="tests/spin-p3.toml"
    ;;
  *)
    echo "unsupported PREVIEW: $PREVIEW" >&2
    exit 2
    ;;
esac

# Preview 3 already defaults to 128; Preview 2 defaults to 1. Prefer the
# existing Wasmtime/e2e knobs, then PRODUCTION.md's recommended 128 for
# Preview 2 candidate soaks (the tree that owns this script) so a reused
# instance can amortize `RouteTable` discovery. Preview 3 omits the flag
# (host default). CI baseline worktrees (PROJECT_ROOT) stay on the host
# default: the 0.3.2 soak baseline panics on a second `Executor::new`
# under reuse.
EXPLICIT_REUSE="${WASMTIME_MAX_INSTANCE_REUSE_COUNT:-${LEPTOS_WASI_MAX_INSTANCE_REUSE:-}}"
REUSE_COUNT=""
if [[ "$HOST" == "wasmtime" ]]; then
  if [[ -n "$EXPLICIT_REUSE" ]]; then
    REUSE_COUNT="$EXPLICIT_REUSE"
  elif [[ "$PREVIEW" == "p2" && "$ROOT" == "$TOOL_ROOT" ]]; then
    REUSE_COUNT=128
  fi
  if [[ -n "$REUSE_COUNT" ]]; then
    echo "instance reuse count: $REUSE_COUNT"
  else
    echo "instance reuse count: host default"
  fi
fi

if [[ "${DRY_RUN:-}" == "1" ]]; then
  if [[ "$HOST" == "wasmtime" ]]; then
    DRY_ARGS=(serve -S cli=y)
    if [[ "$PREVIEW" == "p3" ]]; then
      DRY_ARGS+=(-S p3=y -W component-model-async=y)
    fi
    if [[ -n "$REUSE_COUNT" ]]; then
      DRY_ARGS+=(--max-instance-reuse-count "$REUSE_COUNT")
    fi
    echo "dry-run wasmtime: ${DRY_ARGS[*]}"
  else
    echo "dry-run $HOST: no Wasmtime instance-reuse flag"
  fi
  exit 0
fi

cd "$ROOT"
LEPTOS_OUTPUT_NAME=test-app cargo build \
  --locked \
  --manifest-path tests/test-app/Cargo.toml \
  --target wasm32-wasip2 \
  --release \
  --no-default-features \
  --features "$FEATURES"
cp \
  tests/test-app/target/wasm32-wasip2/release/test_app.wasm \
  "tests/$WASM_NAME"

HOST_LOG="$(mktemp -t leptos-wasi-soak.XXXXXX)"
cleanup() {
  status=$?
  kill "$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
  if [[ $status -ne 0 ]]; then
    cat "$HOST_LOG" >&2
  fi
  rm -f "$HOST_LOG"
  exit "$status"
}

case "$HOST" in
  wasmtime)
    WASMTIME_ARGS=(serve -S cli=y)
    if [[ "$PREVIEW" == "p3" ]]; then
      WASMTIME_ARGS+=(
        -S p3=y
        -W component-model-async=y
      )
    fi
    if [[ -n "$REUSE_COUNT" ]]; then
      WASMTIME_ARGS+=(--max-instance-reuse-count "$REUSE_COUNT")
    fi
    WASMTIME_ARGS+=(
      --dir tests/test-app/static::/static
      "tests/$WASM_NAME"
      --addr "127.0.0.1:$PORT"
    )
    wasmtime "${WASMTIME_ARGS[@]}" >"$HOST_LOG" 2>&1 &
    ;;
  spin)
    spin up \
      -f "$SPIN_MANIFEST" \
      --listen "127.0.0.1:$PORT" \
      >"$HOST_LOG" 2>&1 &
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

SERVER_PID=$!
trap cleanup EXIT INT TERM

URL="http://127.0.0.1:$PORT/api/get_test"
for _ in $(seq 1 60); do
  if curl --fail --silent --max-time 2 "$URL" >/dev/null; then
    break
  fi
  sleep 1
done
curl --fail --silent --max-time 2 "$URL" >/dev/null

PROBE=(
  python3 "$TOOL_ROOT/scripts/load_runtime.py"
  "$URL"
  --duration "$DURATION"
  --concurrency "$CONCURRENCY"
  --pid "$SERVER_PID"
)
if [[ -n "$OUTPUT" ]]; then
  "${PROBE[@]}" | tee "$OUTPUT"
else
  "${PROBE[@]}"
fi

if [[ "$PREVIEW" == "p2" && "$CHECK_POLLABLES" == "1" ]]; then
  POLLABLE_DEPTH="$(
    curl --fail --silent --max-time 5 \
      "http://127.0.0.1:$PORT/api/pollable_depth"
  )"
  if [[ "$POLLABLE_DEPTH" != "0" ]]; then
    echo "orphaned WASIp2 pollables: $POLLABLE_DEPTH" >&2
    exit 1
  fi
  POLLABLE_RESULT='{"pollable_queue_depth":0}'
  if [[ -n "$OUTPUT" ]]; then
    printf '%s\n' "$POLLABLE_RESULT" \
      >"${OUTPUT%.json}-pollables.json"
  else
    printf '%s\n' "$POLLABLE_RESULT"
  fi
fi

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
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
  "$ROOT/scripts/build-middleware-fixture.sh"
  SERVER_COMPONENT="target/wasm32-wasip2/release/counter-middleware.wasm"
  "$ROOT/scripts/compose-middleware.sh" \
    "target/wasm32-wasip2/release/counter.wasm" \
    "$SERVER_COMPONENT" \
    "$ROOT/tests/middleware-fixture.wasm"
fi

case "$HOST" in
  wasmtime)
    if [[ "$MIDDLEWARE" == "1" ]]; then
      "$ROOT/scripts/audit-middleware-manifests.py" --wasmtime
    fi
    wasmtime serve \
      -W component-model-async=y \
      -S p3=y \
      -S cli=y \
      -S http=y \
      --dir="$PWD/target/site::/site" \
      --env=LEPTOS_OUTPUT_NAME=counter \
      --env=LEPTOS_SITE_ROOT=/site \
      --env=LEPTOS_SITE_PKG_DIR=pkg \
      --addr "127.0.0.1:$PORT" \
      "$SERVER_COMPONENT" &
    ;;
  spin)
    if [[ "$MIDDLEWARE" == "1" ]]; then
      "$ROOT/scripts/audit-middleware-manifests.py" --spin
      spin up \
        --file spin.middleware-vnext.toml \
        --listen "127.0.0.1:$PORT" &
    else
      spin up --listen "127.0.0.1:$PORT" &
    fi
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

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

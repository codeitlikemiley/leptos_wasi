#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOST="${HOST:-wasmtime}"
PORT="${PORT:-3000}"

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

case "$HOST" in
  wasmtime)
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
      target/wasm32-wasip2/release/counter.wasm &
    ;;
  spin)
    spin up --listen "127.0.0.1:$PORT" &
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
BASE_URL="http://127.0.0.1:$PORT" npm test

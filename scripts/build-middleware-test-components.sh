#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

LEPTOS_OUTPUT_NAME=test-app cargo build \
  --locked \
  --manifest-path "$ROOT/tests/test-app/Cargo.toml" \
  --target wasm32-wasip2 \
  --release \
  --no-default-features \
  --features wasip3,islands-router

install -m 0644 \
  "$ROOT/tests/test-app/target/wasm32-wasip2/release/test_app.wasm" \
  "$ROOT/tests/test-app-p3.wasm"

"$ROOT/scripts/build-middleware-fixture.sh"

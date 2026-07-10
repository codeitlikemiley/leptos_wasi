#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/tests/middleware-fixture/Cargo.toml"
ARTIFACT="$ROOT/tests/middleware-fixture/target/wasm32-wasip2/release/leptos_wasi_middleware_fixture.wasm"
OUTPUT="$ROOT/tests/middleware-fixture.wasm"

cargo build \
  --locked \
  --manifest-path "$MANIFEST" \
  --target wasm32-wasip2 \
  --release

install -m 0644 "$ARTIFACT" "$OUTPUT"

if command -v wasm-tools >/dev/null 2>&1; then
  WIT="$(wasm-tools component wit "$OUTPUT")"
  printf '%s\n' "$WIT" | rg -q 'import wasi:http/handler@0\.3\.0-rc-2026-03-15'
  printf '%s\n' "$WIT" | rg -q 'export wasi:http/handler@0\.3\.0-rc-2026-03-15'
fi

echo "built protocol-only middleware fixture: $OUTPUT"

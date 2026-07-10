#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-3000}"

"$ROOT/scripts/audit-middleware-manifests.py" --wasmtime --composition-tools
"$ROOT/scripts/build-middleware-test-components.sh"
"$ROOT/scripts/compose-middleware.sh" \
  "$ROOT/tests/test-app-p3.wasm" \
  "$ROOT/tests/test-app-p3-middleware.wasm" \
  "$ROOT/tests/middleware-fixture.wasm"

exec wasmtime serve \
  -W component-model-async=y \
  -S p3=y \
  -S cli=y \
  -S http=y \
  --dir="$ROOT/tests/test-app/static::/static" \
  --addr "127.0.0.1:$PORT" \
  "$ROOT/tests/test-app-p3-middleware.wasm"

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${HOST:-all}"
cd "$ROOT"

./scripts/audit-middleware-manifests.py
./scripts/build-middleware-test-components.sh
./scripts/compose-middleware.sh \
  tests/test-app-p3.wasm \
  tests/test-app-p3-middleware.wasm \
  tests/middleware-fixture.wasm

case "$HOST" in
  all)
    ./scripts/audit-middleware-manifests.py --spin --wasmtime
    FILTER="experimental_middleware"
    ;;
  wasmtime)
    ./scripts/audit-middleware-manifests.py --wasmtime
    FILTER="experimental_middleware_wasmtime_wasip3"
    ;;
  spin)
    ./scripts/audit-middleware-manifests.py --spin
    FILTER="experimental_middleware_spin_wasip3"
    ;;
  *)
    echo "unsupported HOST: $HOST" >&2
    exit 2
    ;;
esac

cargo test --locked --test e2e "$FILTER" \
  -- --ignored --nocapture --test-threads=1

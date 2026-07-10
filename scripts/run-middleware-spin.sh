#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-3000}"

"$ROOT/scripts/audit-middleware-manifests.py" --spin
"$ROOT/scripts/build-middleware-test-components.sh"

exec spin up \
  --file "$ROOT/tests/spin-p3-middleware-vnext.toml" \
  --listen "127.0.0.1:$PORT"

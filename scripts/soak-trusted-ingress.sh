#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DURATION="${DURATION:-600}"
CONCURRENCY="${CONCURRENCY:-100}"
HOST="${HOST:-wasmtime}"
SOAK_DIR="${AUTHZ_FULL_CHAIN_SOAK_DIR:-$ROOT/target/authz-full-chain-soak}"
run_status=0
AUTHENTICATION_MODE=trusted_ingress AUTHZ=1 MIDDLEWARE=0 HOST="$HOST" \
  AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 AUTHZ_FULL_CHAIN_SOAK=1 \
  TRUSTED_INGRESS_HEALTH_CHECKS=1 \
  AUTHZ_FULL_CHAIN_SOAK_DURATION="$DURATION" \
  AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY="$CONCURRENCY" \
  AUTHZ_FULL_CHAIN_SOAK_DIR="$SOAK_DIR" \
  "$ROOT/scripts/run-trusted-ingress-browser.sh" || run_status=$?

report="$SOAK_DIR/result.json"
if [[ ! -f "$report" ]]; then
  echo "trusted-ingress soak did not produce $report" >&2
  [[ "$run_status" -ne 0 ]] && exit "$run_status"
  exit 2
fi
summary_status=0
python3 "$ROOT/scripts/check-trusted-load-soak.py" \
  "$report" --output "$SOAK_DIR/summary.json" || summary_status=$?
if [[ "$run_status" -ne 0 || "$summary_status" -ne 0 ]]; then
  exit 1
fi

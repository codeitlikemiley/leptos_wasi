#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESULTS="${RESULTS:-$ROOT/target/trusted-ingress-benchmark}"
REPETITIONS="${REPETITIONS:-5}"
mkdir -p "$RESULTS"

run_profile() {
  local profile="$1" path="$2" policy="$3" method="$4"
  local repetition
  for repetition in $(seq 1 "$REPETITIONS"); do
    AUTHENTICATION_MODE=trusted_ingress AUTHZ=1 MIDDLEWARE=0 HOST=wasmtime \
      TRUSTED_INGRESS_POLICY_ENABLED="$policy" \
      AUTHZ_FULL_CHAIN_BENCHMARK=1 AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 \
      AUTHZ_FULL_CHAIN_BENCHMARK_PATH="$path" \
      AUTHZ_FULL_CHAIN_BENCHMARK_METHOD="$method" \
      AUTHZ_FULL_CHAIN_BENCHMARK_DIR="$RESULTS/$profile/$repetition" \
      "$ROOT/scripts/run-trusted-ingress-browser.sh"
  done
}

run_profile proxy-baseline / false GET
run_profile edge-policy / true GET
run_profile anonymous-ssr / true GET
run_profile cedar /api/authorize_cedar true POST
run_profile spicedb /api/authorize_relationship true POST
run_profile hybrid /api/increment_count true POST

python3 "$ROOT/scripts/summarize-trusted-ingress-benchmark.py" "$RESULTS"

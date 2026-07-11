#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESULTS="${RESULTS:-$ROOT/target/trusted-ingress-tuning}"
REPETITIONS="${REPETITIONS:-5}"
REQUESTS="${REQUESTS:-5000}"
WARMUP_REQUESTS="${WARMUP_REQUESTS:-500}"
REUSE_CONCURRENCY_VALUES=(8 16 32)
REUSE_TOTAL_VALUES=(128 512 4096)
LOAD_CONCURRENCY_VALUES=(1 8 16 32 64 100)

if [[ "${TUNING_QUICK:-0}" == "1" ]]; then
  REUSE_CONCURRENCY_VALUES=(16)
  REUSE_TOTAL_VALUES=(512)
  LOAD_CONCURRENCY_VALUES=(16 100)
  REPETITIONS=1
  REQUESTS=500
  WARMUP_REQUESTS=100
fi

mkdir -p "$RESULTS"
for reuse_concurrency in "${REUSE_CONCURRENCY_VALUES[@]}"; do
  for reuse_total in "${REUSE_TOTAL_VALUES[@]}"; do
    candidate="reuse-${reuse_total}-concurrent-${reuse_concurrency}"
    for load_concurrency in "${LOAD_CONCURRENCY_VALUES[@]}"; do
      for repetition in $(seq 1 "$REPETITIONS"); do
        output="$RESULTS/$candidate/concurrency-$load_concurrency/$repetition"
        if ! TERMINAL_MAX_INSTANCE_REUSE_COUNT="$reuse_total" \
          TERMINAL_MAX_INSTANCE_CONCURRENT_REUSE_COUNT="$reuse_concurrency" \
          AUTHENTICATION_MODE=trusted_ingress AUTHZ=1 HOST=wasmtime \
          AUTHZ_FULL_CHAIN_BENCHMARK=1 AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 \
          AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS="$REQUESTS" \
          AUTHZ_FULL_CHAIN_BENCHMARK_WARMUP_REQUESTS="$WARMUP_REQUESTS" \
          AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY="$load_concurrency" \
          AUTHZ_FULL_CHAIN_SCENARIO="$ROOT/tests/trusted-ingress/scenarios/hybrid-direct.toml" \
          AUTHZ_FULL_CHAIN_BENCHMARK_DIR="$output" \
          "$ROOT/scripts/run-trusted-ingress-browser.sh"; then
          mkdir -p "$output"
          printf '%s\n' rejected >"$output/rejected"
        fi
      done
    done
  done
done

python3 "$ROOT/scripts/select-trusted-ingress-lifecycle.py" \
  "$RESULTS" "$REPETITIONS" "${LOAD_CONCURRENCY_VALUES[@]}"

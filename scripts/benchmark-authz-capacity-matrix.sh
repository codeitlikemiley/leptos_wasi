#!/usr/bin/env bash

set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_ROOT="${AUTHZ_CAPACITY_MATRIX_DIR:-${ROOT}/target/authz-capacity-matrix}"
REQUESTS="${AUTHZ_CAPACITY_MATRIX_REQUESTS:-500}"
WARMUP_REQUESTS="${AUTHZ_CAPACITY_MATRIX_WARMUP_REQUESTS:-100}"
CONCURRENCY="${AUTHZ_CAPACITY_MATRIX_CONCURRENCY:-100}"

# These are offered rates, not active concurrency. The report records both so
# a capacity knee is not confused with the 100-active-request gate.
RATES=(25 50 75 100 125 150 200)
mkdir -p "${OUTPUT_ROOT}"

for profile in domain coarse; do
  coarse_flag=0
  if [[ "${profile}" == "coarse" ]]; then
    coarse_flag=1
  fi
  for rate in "${RATES[@]}"; do
    run_dir="${OUTPUT_ROOT}/${profile}-${rate}rps"
    mkdir -p "${run_dir}"
    echo "capacity-matrix profile=${profile} rate=${rate} concurrency=${CONCURRENCY}"
    AUTHZ_COARSE_PEP="${coarse_flag}" \
    MIDDLEWARE_DIAGNOSTICS=1 \
    AUTHZ_FULL_CHAIN_BENCHMARK=1 \
    AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 \
    AUTHZ_FULL_CHAIN_BENCHMARK_DIR="${run_dir}" \
    AUTHZ_FULL_CHAIN_BENCHMARK_REQUESTS="${REQUESTS}" \
    AUTHZ_FULL_CHAIN_BENCHMARK_WARMUP_REQUESTS="${WARMUP_REQUESTS}" \
    AUTHZ_FULL_CHAIN_BENCHMARK_CONCURRENCY="${CONCURRENCY}" \
    AUTHZ_FULL_CHAIN_BENCHMARK_RATE="${rate}" \
    AUTHZ_TEST_ONLY=1 \
    AUTHZ=1 \
    MIDDLEWARE=1 \
    HOST=wasmtime \
      bash "${ROOT}/scripts/run-authz-browser.sh" \
      >"${run_dir}/runner.log" 2>&1
    status=$?
    printf '%s\n' "${status}" >"${run_dir}/exit-status"
  done
done

echo "capacity matrix written to ${OUTPUT_ROOT}"

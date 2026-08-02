#!/usr/bin/env bash

set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_ROOT="${AUTHZ_CAPACITY_MATRIX_DIR:-${ROOT}/target/authz-capacity-matrix}"
REQUESTS="${AUTHZ_CAPACITY_MATRIX_REQUESTS:-500}"
WARMUP_REQUESTS="${AUTHZ_CAPACITY_MATRIX_WARMUP_REQUESTS:-100}"
CONCURRENCY="${AUTHZ_CAPACITY_MATRIX_CONCURRENCY:-100}"

# These are offered rates, not active concurrency. The report records both so
# a capacity knee is not confused with the 100-active-request gate.
read -r -a RATES <<<"${AUTHZ_CAPACITY_MATRIX_RATES:-25 50 75 100 125 150 200}"
read -r -a PROFILES <<<"${AUTHZ_CAPACITY_MATRIX_PROFILES:-domain coarse}"

if ((${#RATES[@]} == 0)) || ((${#PROFILES[@]} == 0)); then
  echo "capacity matrix requires at least one rate and profile" >&2
  exit 2
fi
for rate in "${RATES[@]}"; do
  [[ "${rate}" =~ ^[0-9]+$ ]] || {
    echo "capacity matrix rate must be a non-negative integer: ${rate}" >&2
    exit 2
  }
done
for profile in "${PROFILES[@]}"; do
  [[ "${profile}" == "domain" || "${profile}" == "coarse" ]] || {
    echo "capacity matrix profile must be domain or coarse: ${profile}" >&2
    exit 2
  }
done
mkdir -p "${OUTPUT_ROOT}"

results=()
floor_failures=()
lowest_rate="${RATES[0]}"
for rate in "${RATES[@]}"; do
  ((rate < lowest_rate)) && lowest_rate="${rate}"
done

for profile in "${PROFILES[@]}"; do
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
    results+=("${profile} ${rate} ${status}")
    if ((rate == lowest_rate)) && ((status != 0)); then
      floor_failures+=("${profile} at ${rate} rps")
    fi
  done
done

# A capacity sweep is meant to break at high offered rates - that is how the
# knee is found - so a failure above the floor is data, not an error. A
# failure at the LOWEST rate means nothing ran at all, which is.
echo
echo "capacity matrix results (profile rate exit-status):"
printf '  %s\n' "${results[@]}"
echo "capacity matrix written to ${OUTPUT_ROOT}"

if ((${#floor_failures[@]} > 0)); then
  echo >&2
  echo "capacity matrix failed at its lowest offered rate: ${floor_failures[*]}" >&2
  echo "this is not a capacity knee; no useful curve was produced" >&2
  exit 1
fi

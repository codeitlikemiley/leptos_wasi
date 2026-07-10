#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

RUNS="${RUNS:-5}"
DURATION="${DURATION:-30}"
WARMUP_DURATION="${WARMUP_DURATION:-5}"
COOLDOWN_DURATION="${COOLDOWN_DURATION:-2}"
CONCURRENCY="${CONCURRENCY:-100}"
PORT="${PORT:-3185}"
OUTPUT_DIR="${OUTPUT_DIR:-${ROOT}/target/middleware-performance}"
WORKLOAD="${WORKLOAD:-representative}"
DIAGNOSTIC_ROUTE="${DIAGNOSTIC_ROUTE:-/api/get_test}"
CANDIDATE_COMPONENT="${CANDIDATE_COMPONENT:-secure-defaults}"

case "${CANDIDATE_COMPONENT}" in
  passthrough | request-id | security-headers | cors | authn-policy | secure-defaults) ;;
  *)
    echo "unsupported CANDIDATE_COMPONENT: ${CANDIDATE_COMPONENT}" >&2
    exit 2
    ;;
esac

if (( RUNS < 5 )); then
  echo "RUNS must be at least 5" >&2
  exit 2
fi

cd "${ROOT}"
if [[ -n "${MIDDLEWARE_BENCHMARK_ARTIFACT_DIR:-}" ]]; then
  LEPTOS_OUTPUT_NAME=test-app cargo build \
    --locked \
    --manifest-path tests/test-app/Cargo.toml \
    --target wasm32-wasip2 \
    --release \
    --no-default-features \
    --features wasip3,islands-router
  install -m 0644 \
    tests/test-app/target/wasm32-wasip2/release/test_app.wasm \
    tests/test-app-p3.wasm
  install -m 0644 \
    "${MIDDLEWARE_BENCHMARK_ARTIFACT_DIR}/${CANDIDATE_COMPONENT}.wasm" \
    "tests/middleware-artifacts/${CANDIDATE_COMPONENT}.wasm"
else
  ./scripts/build-middleware-test-components.sh
fi
./scripts/compose-middleware.sh \
  tests/test-app-p3.wasm \
  "tests/test-app-p3-${CANDIDATE_COMPONENT}.wasm" \
  "tests/middleware-artifacts/${CANDIDATE_COMPONENT}.wasm"

WASMTIME_BIN="$(resolve_middleware_tool WASMTIME_BIN wasmtime "$(middleware_lock_value wasmtime_version)")"
OHA_BIN="$(resolve_middleware_tool OHA_BIN oha "$(middleware_lock_value oha_version)")"
mkdir -p "${OUTPUT_DIR}"

case "${WORKLOAD}" in
  representative)
    URL_FILE="${OUTPUT_DIR}/representative-urls.txt"
    awk -v base="http://127.0.0.1:${PORT}" \
      '{ print base $0 }' \
      tests/middleware/performance-routes.txt >"${URL_FILE}"
    LOAD_TARGET=(--urls-from-file "${URL_FILE}")
    ;;
  tiny)
    LOAD_TARGET=("http://127.0.0.1:${PORT}${DIAGNOSTIC_ROUTE}")
    ;;
  *)
    echo "unsupported WORKLOAD: ${WORKLOAD}" >&2
    exit 2
    ;;
esac

CURRENT_PID=""
cleanup() {
  if [[ -n "${CURRENT_PID}" ]] && kill -0 "${CURRENT_PID}" 2>/dev/null; then
    kill "${CURRENT_PID}" 2>/dev/null || true
    wait "${CURRENT_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

run_profile() {
  local profile="$1"
  local run="$2"
  local component
  local -a args
  component="tests/test-app-p3.wasm"
  args=(
    serve
    -W component-model-async=y
    -S p3=y
    -S cli=y
    -S http=y
  )
  if [[ "${profile}" == "fused" ]]; then
    component="tests/test-app-p3-${CANDIDATE_COMPONENT}.wasm"
    if [[ "${CANDIDATE_COMPONENT}" == "cors" || "${CANDIDATE_COMPONENT}" == "secure-defaults" ]]; then
      args+=(
      --env=WASI_MIDDLEWARE_CORS_ORIGINS=http://127.0.0.1:3110
      --env=WASI_MIDDLEWARE_CORS_METHODS=GET,HEAD,POST
      --env=WASI_MIDDLEWARE_CORS_HEADERS=content-type,authorization,x-request-id
      --env=WASI_MIDDLEWARE_CORS_ALLOW_CREDENTIALS=false
      )
    fi
    if [[ "${CANDIDATE_COMPONENT}" == "authn-policy" || "${CANDIDATE_COMPONENT}" == "secure-defaults" ]]; then
      args+=(
      -S inherit-network=y
      --env=WASI_MIDDLEWARE_AUTHN_BROKER_URL=http://127.0.0.1:3112/authenticate
      --env=WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS=2000
      --env=WASI_MIDDLEWARE_AUTHN_MODE=optional
      --env=WASI_MIDDLEWARE_SERVICE_ID=leptos-wasi-test-app
      --env=WASI_MIDDLEWARE_AUTHN_AUDIENCES=api://leptos-wasi-test-app
      --env=WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT=64
      --env=WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK=true
      )
    fi
  fi
  args+=(
    --dir="${ROOT}/tests/test-app/static::/static"
    --addr "127.0.0.1:${PORT}"
    "${component}"
  )

  local log="${OUTPUT_DIR}/${profile}-${run}.log"
  "${WASMTIME_BIN}" "${args[@]}" >"${log}" 2>&1 &
  CURRENT_PID=$!
  local ready="000"
  for _ in $(seq 1 100); do
    ready="$(curl --silent --max-time 1 --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${PORT}${DIAGNOSTIC_ROUTE}" || true)"
    [[ "${ready}" == "200" ]] && break
    sleep 0.05
  done
  if [[ "${ready}" != "200" ]]; then
    cat "${log}" >&2
    echo "${profile} profile did not become ready" >&2
    exit 1
  fi

  # Exercise Wasmtime's lazy paths and let the service reach steady state
  # before recording tail latency. The warm-up uses the same concurrency and
  # fails the run if it observes an unexpected response.
  NO_COLOR=true "${OHA_BIN}" \
    -z "${WARMUP_DURATION}s" \
    -c "${CONCURRENCY}" \
    -w \
    --http-version 1.1 \
    -t 10s \
    --no-tui \
    --output-format json \
    "${LOAD_TARGET[@]}" \
    >"${OUTPUT_DIR}/${profile}-${run}-warmup.json"

  NO_COLOR=true "${OHA_BIN}" \
    -z "${DURATION}s" \
    -c "${CONCURRENCY}" \
    -w \
    --http-version 1.1 \
    -t 10s \
    --no-tui \
    --output-format json \
    "${LOAD_TARGET[@]}" \
    >"${OUTPUT_DIR}/${profile}-${run}.json"

  kill "${CURRENT_PID}" 2>/dev/null || true
  wait "${CURRENT_PID}" 2>/dev/null || true
  CURRENT_PID=""
  sleep "${COOLDOWN_DURATION}"
}

for run in $(seq 1 "${RUNS}"); do
  if (( run % 2 == 1 )); then
    run_profile unwrapped "${run}"
    run_profile fused "${run}"
  else
    run_profile fused "${run}"
    run_profile unwrapped "${run}"
  fi
done

comparison_args=()
for run in $(seq 1 "${RUNS}"); do
  comparison_args+=(
    --baseline "${OUTPUT_DIR}/unwrapped-${run}.json"
    --candidate "${OUTPUT_DIR}/fused-${run}.json"
  )
done
python3 scripts/compare_middleware_profiles.py "${comparison_args[@]}" \
  --candidate-label "${CANDIDATE_COMPONENT}" \
  --workload "${WORKLOAD}" \
  | tee "${OUTPUT_DIR}/comparison.json"

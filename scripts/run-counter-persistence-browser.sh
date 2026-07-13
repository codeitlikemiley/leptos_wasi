#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

counter_directory="${ROOT}/examples/counter"
store_manifest="${ROOT}/examples/production-counter/store/Cargo.toml"
store_binary="${ROOT}/examples/production-counter/store/target/release/leptos-wasi-production-counter-store"
result_directory="${ROOT}/target/counter-persistence-browser"
runtime_list="${PERSISTENCE_HOSTS:-wasmtime spin}"
database_path="${result_directory}/counter.sqlite3"
store_log="${result_directory}/store.log"
runtime_pid=""
store_pid=""

available_port() {
  python3 -c 'import socket; stream=socket.socket(); stream.bind(("127.0.0.1", 0)); print(stream.getsockname()[1]); stream.close()'
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
  [[ -n "${runtime_pid}" ]] && kill "${runtime_pid}" 2>/dev/null || true
  [[ -n "${runtime_pid}" ]] && wait "${runtime_pid}" 2>/dev/null || true
  [[ -n "${store_pid}" ]] && kill "${store_pid}" 2>/dev/null || true
  [[ -n "${store_pid}" ]] && wait "${store_pid}" 2>/dev/null || true
  exit "${status}"
}
trap cleanup EXIT INT TERM

mkdir -p "${result_directory}"
rm -f "${database_path}" "${database_path}-shm" "${database_path}-wal"
cargo build --locked --release --manifest-path "${store_manifest}"
DATABASE_URL="sqlite://${database_path}" \
COUNTER_STORE_LISTEN_ADDR=127.0.0.1:4040 \
  "${store_binary}" >"${store_log}" 2>&1 &
store_pid=$!
for _attempt in $(seq 1 100); do
  curl --silent --fail http://127.0.0.1:4040/health >/dev/null 2>&1 && break
  kill -0 "${store_pid}" 2>/dev/null || {
    cat "${store_log}" >&2
    exit 1
  }
  sleep 0.05
done
curl --silent --fail http://127.0.0.1:4040/health >/dev/null

cd "${counter_directory}"
cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib \
  --target wasm32-wasip2 --release --no-default-features --features ssr
make verify-split

wasmtime_bin="$(resolve_middleware_tool \
  WASMTIME_BIN wasmtime "$(middleware_lock_value wasmtime_version)")"
spin_bin=""
if [[ " ${runtime_list} " == *" spin "* ]]; then
  spin_bin="$(resolve_spin_main_tool)"
fi

run_browser_case() {
  local runtime="$1"
  local port
  local runtime_log="${result_directory}/${runtime}.log"
  port="$(available_port)"
  case "${runtime}" in
    wasmtime)
      "${wasmtime_bin}" serve \
        -W component-model-async=y \
        -S p3=y \
        -S cli=y \
        -S http=y \
        -S inherit-network=y \
        --dir="${counter_directory}/target/site::/site" \
        --env=LEPTOS_OUTPUT_NAME=counter \
        --env=LEPTOS_SITE_ROOT=/site \
        --env=LEPTOS_SITE_PKG_DIR=pkg \
        --env=COUNTER_STORE_URL=http://127.0.0.1:4040 \
        --env=COUNTER_ID=demo \
        --addr "127.0.0.1:${port}" \
        "${counter_directory}/target/wasm32-wasip2/release/counter.wasm" \
        >"${runtime_log}" 2>&1 &
      ;;
    spin)
      PORT="${port}" "${spin_bin}" up \
        --file "${counter_directory}/spin.toml" \
        --listen "127.0.0.1:${port}" >"${runtime_log}" 2>&1 &
      ;;
  esac
  runtime_pid=$!
  for _attempt in $(seq 1 100); do
    curl --silent --fail "http://127.0.0.1:${port}/" >/dev/null 2>&1 && break
    kill -0 "${runtime_pid}" 2>/dev/null || {
      cat "${runtime_log}" >&2
      exit 1
    }
    sleep 0.05
  done
  curl --silent --fail "http://127.0.0.1:${port}/" >/dev/null

  cd "${ROOT}/tests/browser"
  if [[ ! -x node_modules/.bin/playwright ]]; then
    npm ci
  fi
  BASE_URL="http://127.0.0.1:${port}" PERSISTENCE=1 \
    npx playwright test --grep "SQLite counter survives"
  kill "${runtime_pid}" 2>/dev/null || true
  wait "${runtime_pid}" 2>/dev/null || true
  runtime_pid=""
  cd "${counter_directory}"
}

runtime_count=0
for runtime in ${runtime_list}; do
  case "${runtime}" in
    wasmtime | spin) ;;
    *)
      echo "unsupported persistence host: ${runtime}" >&2
      exit 2
      ;;
  esac
  run_browser_case "${runtime}"
  runtime_count=$((runtime_count + 1))
done

value="$(curl --silent --fail \
  http://127.0.0.1:4040/v1/counters/demo)"
[[ "${value}" == "{\"value\":${runtime_count}}" ]] || {
  echo "unexpected final persisted counter value: ${value}" >&2
  exit 1
}
kill "${store_pid}" 2>/dev/null || true
wait "${store_pid}" 2>/dev/null || true
store_pid=""

outage_port="$(available_port)"
outage_log="${result_directory}/store-outage.log"
"${wasmtime_bin}" serve \
  -W component-model-async=y \
  -S p3=y \
  -S cli=y \
  -S http=y \
  -S inherit-network=y \
  --dir="${counter_directory}/target/site::/site" \
  --env=LEPTOS_OUTPUT_NAME=counter \
  --env=LEPTOS_SITE_ROOT=/site \
  --env=LEPTOS_SITE_PKG_DIR=pkg \
  --env=COUNTER_STORE_URL=http://127.0.0.1:4040 \
  --env=COUNTER_ID=demo \
  --addr "127.0.0.1:${outage_port}" \
  "${counter_directory}/target/wasm32-wasip2/release/counter.wasm" \
  >"${outage_log}" 2>&1 &
runtime_pid=$!
for _attempt in $(seq 1 100); do
  curl --silent --fail "http://127.0.0.1:${outage_port}/" >/dev/null 2>&1 && break
  sleep 0.05
done
outage_status="$(curl --silent \
  --dump-header "${result_directory}/store-outage.headers" \
  --output "${result_directory}/store-outage.body" \
  --write-out '%{http_code}' \
  --header 'content-type: application/x-www-form-urlencoded' \
  --data '' \
  "http://127.0.0.1:${outage_port}/api/get_count")"
[[ "${outage_status}" == "503" ]]
grep -Eiq '^cache-control: no-store' \
  "${result_directory}/store-outage.headers"
if grep -Eiq 'connection|127\.0\.0\.1|internal|transport' \
  "${result_directory}/store-outage.body"; then
  echo "counter outage response disclosed an internal error" >&2
  exit 1
fi
kill "${runtime_pid}" 2>/dev/null || true
wait "${runtime_pid}" 2>/dev/null || true
runtime_pid=""
echo "SQLite persistence passed for: ${runtime_list}."

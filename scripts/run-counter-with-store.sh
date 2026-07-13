#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

runtime="${1:-}"
counter_directory="${2:-${ROOT}/examples/counter}"
store_manifest="${ROOT}/examples/production-counter/store/Cargo.toml"
store_binary="${ROOT}/examples/production-counter/store/target/release/leptos-wasi-production-counter-store"
database_path="${COUNTER_STORE_DATABASE_PATH:-${ROOT}/data/counter.sqlite3}"
store_origin="http://127.0.0.1:4040"
store_log="${ROOT}/target/counter-store.log"
port="${PORT:-3000}"
store_pid=""

case "${runtime}" in
  spin | wasmtime) ;;
  *)
    echo "usage: $0 <spin|wasmtime> [counter-directory]" >&2
    exit 2
    ;;
esac

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [[ -n "${store_pid}" ]]; then
    kill "${store_pid}" 2>/dev/null || true
    wait "${store_pid}" 2>/dev/null || true
  fi
  exit "${status}"
}
trap cleanup EXIT INT TERM

if curl --silent --fail "${store_origin}/health" >/dev/null 2>&1; then
  echo "Reusing counter store at ${store_origin}."
else
  mkdir -p "$(dirname "${database_path}")" "$(dirname "${store_log}")"
  cargo build --locked --release --manifest-path "${store_manifest}"
  DATABASE_URL="sqlite://${database_path}" \
  COUNTER_STORE_LISTEN_ADDR="127.0.0.1:4040" \
    "${store_binary}" >"${store_log}" 2>&1 &
  store_pid=$!
  for _attempt in $(seq 1 100); do
    if curl --silent --fail "${store_origin}/health" >/dev/null 2>&1; then
      break
    fi
    if ! kill -0 "${store_pid}" 2>/dev/null; then
      echo "counter store exited during startup; see ${store_log}" >&2
      exit 1
    fi
    sleep 0.05
  done
  curl --silent --fail "${store_origin}/health" >/dev/null || {
    echo "counter store did not become ready; see ${store_log}" >&2
    exit 1
  }
  echo "Counter state persists in ${database_path}."
fi

case "${runtime}" in
  spin)
    PORT="${port}" "${ROOT}/scripts/run-spin-main.sh" \
      "${counter_directory}/spin.toml"
    ;;
  wasmtime)
    wasmtime_bin="$(resolve_middleware_tool \
      WASMTIME_BIN \
      wasmtime \
      "$(middleware_lock_value wasmtime_version)")"
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
      --env=COUNTER_STORE_URL="${store_origin}" \
      --env=COUNTER_ID=demo \
      --addr "127.0.0.1:${port}" \
      "${counter_directory}/target/wasmtime/wasm32-wasip2/release/counter.wasm"
    ;;
esac

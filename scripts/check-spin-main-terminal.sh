#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

manifest="${1:-${ROOT}/tests/spin-p3.toml}"
port="${PORT:-19120}"
spin_bin="$(resolve_spin_main_tool)"
log="$(mktemp -t leptos-wasi-spin-main.XXXXXX)"
pid=""
cleanup() {
  [[ -n "${pid}" ]] && kill "${pid}" >/dev/null 2>&1 || true
  [[ -n "${pid}" ]] && wait "${pid}" >/dev/null 2>&1 || true
  rm -f "${log}"
}
trap cleanup EXIT INT TERM

"${spin_bin}" up --file "${manifest}" --listen "127.0.0.1:${port}" >"${log}" 2>&1 &
pid=$!
status="000"
for _ in $(seq 1 200); do
  if ! kill -0 "${pid}" >/dev/null 2>&1; then
    cat "${log}" >&2
    exit 1
  fi
  status="$(curl --silent --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${port}/static-page" || true)"
  [[ "${status}" == "200" ]] && break
  sleep 0.05
done
[[ "${status}" == "200" ]]
curl --fail --silent "http://127.0.0.1:${port}/static/app.css" >/dev/null
echo "pinned Spin main serves final wasi:http@0.3.0 terminal components"

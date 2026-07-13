#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

manifest="${1:-${ROOT}/tests/spin-p3-middleware-composed.toml}"
port="${PORT:-19121}"
spin_bin="$(resolve_spin_main_tool)"
log="$(mktemp -t leptos-wasi-spin-composed.XXXXXX)"
pid=""
cleanup() {
  [[ -n "${pid}" ]] && kill "${pid}" >/dev/null 2>&1 || true
  [[ -n "${pid}" ]] && wait "${pid}" >/dev/null 2>&1 || true
  rm -f "${log}"
}
trap cleanup EXIT INT TERM

"${spin_bin}" up --file "${manifest}" --listen "127.0.0.1:${port}" >"${log}" 2>&1 &
pid=$!
for _ in $(seq 1 200); do
  kill -0 "${pid}" >/dev/null 2>&1 || break
  curl --silent --max-time 1 --output /dev/null "http://127.0.0.1:${port}/static-page" || true
  if grep -Eq 'cpu_time_last_entry|called `Option::unwrap\(\)`|worker panicked' "${log}"; then
    echo "confirmed pinned Spin main CPU-metrics panic for composed final-WASI handlers"
    exit 0
  fi
  sleep 0.05
done
cat "${log}" >&2
echo "Spin composed-handler behavior changed; refresh the compatibility contract" >&2
exit 1

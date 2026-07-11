#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

manifest="${SPIN_MANIFEST:?SPIN_MANIFEST is required}"
middleware="${MIDDLEWARE:-0}"
authz="${AUTHZ:-0}"
[[ "${authz}" == "0" ]] || {
  echo "Spin-main authorization runs must use the trusted-ingress runner" >&2
  exit 2
}

if [[ "${middleware}" == "1" ]]; then
  port="${PORT:-3110}"
  [[ "${SPIN_DIAGNOSTIC_NO_DEFAULT_FEATURES:-0}" == "1" ]] || {
    echo "portable middleware on Spin main requires the explicit no-default-features diagnostic binary" >&2
    exit 2
  }
else
  port="${PORT:-3000}"
fi

spin_bin="$(resolve_spin_main_tool)"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/leptos-wasi-spin-browser.XXXXXX")"
pid=""
cleanup() {
  status=$?
  [[ -n "${pid}" ]] && kill "${pid}" >/dev/null 2>&1 || true
  [[ -n "${pid}" ]] && wait "${pid}" >/dev/null 2>&1 || true
  rm -rf "${temporary}"
  exit "${status}"
}
trap cleanup EXIT INT TERM

"${spin_bin}" up \
  --file "${manifest}" \
  --listen "127.0.0.1:${port}" \
  >"${temporary}/spin.log" 2>&1 &
pid=$!

status="000"
for _ in $(seq 1 200); do
  if ! kill -0 "${pid}" >/dev/null 2>&1; then
    cat "${temporary}/spin.log" >&2
    echo "Spin main exited before browser readiness" >&2
    exit 1
  fi
  status="$(curl --silent --max-time 1 --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:${port}/" || true)"
  [[ "${status}" == "200" ]] && break
  sleep 0.05
done
[[ "${status}" == "200" ]] || {
  cat "${temporary}/spin.log" >&2
  echo "Spin main did not become ready" >&2
  exit 1
}

cd "${ROOT}/tests/browser"
if [[ ! -x node_modules/.bin/playwright ]]; then
  npm ci
fi
BASE_URL="http://127.0.0.1:${port}" MIDDLEWARE="${middleware}" AUTHZ=0 npm test

if grep -Eiq 'authorization:|cookie:|x-wasi-auth-context' "${temporary}/spin.log"; then
  echo "Spin log disclosed protected request metadata" >&2
  exit 1
fi

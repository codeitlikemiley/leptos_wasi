#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

manifest="${1:-${ROOT}/tests/spin-p3.toml}"
[[ -f "${manifest}" ]] || {
  echo "Spin final-WASI canary manifest is unavailable: ${manifest}" >&2
  exit 2
}

SPIN_BIN="$(resolve_middleware_tool SPIN_BIN spin "$(middleware_lock_value spin_stable_version)")"
WASM_TOOLS_BIN="$(resolve_middleware_tool WASM_TOOLS_BIN wasm-tools "$(middleware_lock_value wasm_tools_version)")"
PORT="${PORT:-0}"
log="$(mktemp -t leptos-wasi-spin-final-wasi.XXXXXX)"
pid=""

matches_text() {
  local pattern="$1"
  local text="$2"
  if command -v rg >/dev/null 2>&1; then
    rg -q -- "${pattern}" <<<"${text}"
  else
    grep -Eq -- "${pattern}" <<<"${text}"
  fi
}

matches_file() {
  local pattern="$1"
  local path="$2"
  if command -v rg >/dev/null 2>&1; then
    rg -q -- "${pattern}" "${path}"
  else
    grep -Eq -- "${pattern}" "${path}"
  fi
}

wit="$("${WASM_TOOLS_BIN}" component wit "$(dirname "${manifest}")/$(python3 - "${manifest}" <<'PY'
import pathlib
import sys
import tomllib

manifest = pathlib.Path(sys.argv[1])
with manifest.open("rb") as source:
    value = tomllib.load(source)
trigger = value["trigger"]["http"][0]
print(value["component"][trigger["component"]]["source"])
PY
)")"
if matches_text '0\.3\.0-rc' "${wit}"; then
  echo "Spin canary input is stale and still contains an RC WASI interface" >&2
  exit 2
fi
matches_text 'export wasi:http/handler@0\.3\.0;' "${wit}" || {
  echo "Spin canary input does not export final wasi:http/handler@0.3.0" >&2
  exit 2
}

cleanup() {
  if [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null; then
    kill "${pid}" 2>/dev/null || true
    wait "${pid}" 2>/dev/null || true
  fi
  rm -f "${log}"
}
trap cleanup EXIT INT TERM

"${SPIN_BIN}" up \
  --file "${manifest}" \
  --listen "127.0.0.1:${PORT}" \
  >"${log}" 2>&1 &
pid=$!

for _ in $(seq 1 100); do
  if ! kill -0 "${pid}" 2>/dev/null; then
    break
  fi
  sleep 0.05
done

if kill -0 "${pid}" 2>/dev/null; then
  echo "Spin unexpectedly accepted final wasi:http@0.3.0; promote the compatibility lane and update the lock" >&2
  cat "${log}" >&2
  exit 1
fi

if wait "${pid}"; then
  status=0
else
  status=$?
fi
pid=""

if [[ "${status}" -eq 0 ]]; then
  echo "Spin final-WASI canary exited successfully without serving; expected a linker rejection" >&2
  cat "${log}" >&2
  exit 1
fi

matches_file 'wasi:http/types@0\.3\.0' "${log}" || {
  echo "Spin failed for an unexpected reason; final wasi:http/types@0.3.0 was not mentioned" >&2
  cat "${log}" >&2
  exit 1
}
matches_file 'resource implementation is missing' "${log}" || {
  echo "Spin final-WASI failure changed; update the compatibility evidence before promotion" >&2
  cat "${log}" >&2
  exit 1
}

echo "confirmed: Spin $(middleware_lock_value spin_stable_version) still lacks final wasi:http@0.3.0 host resources"

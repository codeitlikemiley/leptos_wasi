#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

manifest="${1:-${ROOT}/tests/spin-p3.toml}"
port="${PORT:-3000}"
[[ -f "${manifest}" ]] || {
  echo "Spin manifest is unavailable: ${manifest}" >&2
  exit 2
}

spin_bin="$(resolve_spin_main_tool)"
exec "${spin_bin}" up --file "${manifest}" --listen "127.0.0.1:${port}"

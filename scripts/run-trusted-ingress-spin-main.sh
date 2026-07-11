#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

SPIN_BIN="$(resolve_spin_main_tool)" \
HOST=spin \
  exec "${ROOT}/scripts/run-trusted-ingress-browser.sh"

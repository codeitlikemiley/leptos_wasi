#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

available_port() {
  python3 -c 'import socket; stream=socket.socket(); stream.bind(("127.0.0.1", 0)); print(stream.getsockname()[1]); stream.close()'
}

AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 \
AUTHZ_FULL_CHAIN_SOAK=1 \
PORT="${PORT:-$(available_port)}" \
  "${ROOT}/scripts/run-authz-browser.sh"

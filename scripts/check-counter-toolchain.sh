#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/scripts/middleware-common.sh"

runtime="${1:-browser}"
case "${runtime}" in
  browser | spin | wasmtime) ;;
  *)
    echo "usage: $0 [browser|spin|wasmtime]" >&2
    exit 2
    ;;
esac

require_version() {
  local command_name="$1"
  local expected="$2"
  local actual
  require_middleware_command "${command_name}"
  actual="$("${command_name}" --version 2>&1)"
  [[ "${actual}" == *"${expected}"* ]] || {
    echo "${command_name} ${expected} is required; found: ${actual}" >&2
    return 1
  }
}

rust_version="$(rustc --version | awk '{print $2}')"
rust_minor="${rust_version#1.}"
rust_minor="${rust_minor%%.*}"
minimum_rust="$(middleware_lock_value rust_version)"
minimum_minor="${minimum_rust#1.}"
minimum_minor="${minimum_minor%%.*}"
if [[ ! "${rust_minor}" =~ ^[0-9]+$ ]] || ((rust_minor < minimum_minor)); then
  echo "Rust ${minimum_rust} or newer is required; found: ${rust_version}" >&2
  exit 1
fi

rustup target list --installed | grep -Fxq wasm32-wasip2 || {
  echo "missing Rust target wasm32-wasip2; run: rustup target add wasm32-wasip2" >&2
  exit 1
}
rustup target list --installed | grep -Fxq wasm32-unknown-unknown || {
  echo "missing Rust target wasm32-unknown-unknown; run: rustup target add wasm32-unknown-unknown" >&2
  exit 1
}

require_version cargo-leptos "$(middleware_lock_value cargo_leptos_version)"
require_version wasm-bindgen "$(middleware_lock_value wasm_bindgen_cli_version)"

case "${runtime}" in
  spin)
    resolve_spin_main_tool >/dev/null
    ;;
  wasmtime)
    resolve_middleware_tool \
      WASMTIME_BIN \
      wasmtime \
      "$(middleware_lock_value wasmtime_version)" >/dev/null
    ;;
esac

echo "Counter ${runtime} toolchain matches the tested compatibility lock."

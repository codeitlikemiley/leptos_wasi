#!/usr/bin/env bash
# Cloud Agent environment bootstrap for leptos_wasi (leptos-wasi-runtime).
#
# Idempotent: safe to run repeatedly and against a cached/partially prepared
# tree. It installs the exact tool tuple recorded in COMPATIBILITY.md /
# tests/middleware/components.lock.toml, then warms the workspace build.
set -euo pipefail

# Pinned versions. Keep in sync with tests/middleware/components.lock.toml.
WASMTIME_VERSION="46.0.1"
WASM_BINDGEN_VERSION="0.2.126"
CARGO_LEPTOS_VERSION="0.3.7"
TAILWIND_VERSION="v4.2.1"

log() { printf '\n=== %s ===\n' "$1"; }

# sudo only when needed and available (build phase may already run as root).
SUDO=""
if [[ "$(id -u)" -ne 0 ]] && command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
fi

log "System packages (pkg-config + OpenSSL for the reqwest-based test deps)"
if command -v apt-get >/dev/null 2>&1; then
  $SUDO apt-get update -qq
  $SUDO apt-get install -y -qq --no-install-recommends \
    pkg-config libssl-dev curl ca-certificates git
fi

log "Rust toolchain (MSRV is 1.93.0; stable satisfies it) + WASI/browser targets"
if ! command -v rustup >/dev/null 2>&1; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  # shellcheck disable=SC1091
  source "${CARGO_HOME:-$HOME/.cargo}/env"
fi
rustup default stable
rustup target add wasm32-wasip2 wasm32-unknown-unknown

# Install a versioned prebuilt binary to /usr/local/bin unless already present
# at the pinned version. $1=command, $2=expected version substring, $3=URL,
# $4=path of the binary inside the downloaded tarball ("" if the URL is the
# raw binary).
install_prebuilt() {
  local cmd="$1" want="$2" url="$3" inner="$4"
  if command -v "$cmd" >/dev/null 2>&1 && "$cmd" --version 2>&1 | grep -qF "$want"; then
    echo "$cmd $want already installed"
    return 0
  fi
  local tmp
  tmp="$(mktemp -d)"
  if [[ -z "$inner" ]]; then
    curl -sSL -o "$tmp/$cmd" "$url"
    $SUDO install -m 0755 "$tmp/$cmd" "/usr/local/bin/$cmd"
  else
    curl -sSL -o "$tmp/download" "$url"
    tar -xf "$tmp/download" -C "$tmp"
    $SUDO install -m 0755 "$tmp/$inner" "/usr/local/bin/$cmd"
  fi
  rm -rf "$tmp"
  echo "installed $cmd: $("$cmd" --version 2>&1 | head -n1)"
}

log "wasmtime ${WASMTIME_VERSION} (final-WASI correctness reference)"
install_prebuilt wasmtime "$WASMTIME_VERSION" \
  "https://github.com/bytecodealliance/wasmtime/releases/download/v${WASMTIME_VERSION}/wasmtime-v${WASMTIME_VERSION}-x86_64-linux.tar.xz" \
  "wasmtime-v${WASMTIME_VERSION}-x86_64-linux/wasmtime"

log "wasm-bindgen ${WASM_BINDGEN_VERSION} (must match the counter's locked crate)"
install_prebuilt wasm-bindgen "$WASM_BINDGEN_VERSION" \
  "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
  "wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen"

log "cargo-leptos ${CARGO_LEPTOS_VERSION} (islands + lazy WASM splitting)"
install_prebuilt cargo-leptos "$CARGO_LEPTOS_VERSION" \
  "https://github.com/leptos-rs/cargo-leptos/releases/download/v${CARGO_LEPTOS_VERSION}/cargo-leptos-x86_64-unknown-linux-musl.tar.gz" \
  "cargo-leptos-x86_64-unknown-linux-musl/cargo-leptos"

# cargo-leptos would otherwise auto-download a musl-linked Tailwind CLI whose
# loader (/lib/ld-musl-x86_64.so.1) and musl libstdc++ are absent on this
# glibc image, failing the counter's frontend build. cargo-leptos prefers a
# `tailwindcss` already on PATH, so install the matching glibc build there.
log "tailwindcss ${TAILWIND_VERSION} (glibc build, avoids cargo-leptos musl download)"
if ! (command -v tailwindcss >/dev/null 2>&1 && tailwindcss --help 2>&1 | grep -qF "${TAILWIND_VERSION#v}"); then
  tmp="$(mktemp -d)"
  curl -sSL -o "$tmp/tailwindcss" \
    "https://github.com/tailwindlabs/tailwindcss/releases/download/${TAILWIND_VERSION}/tailwindcss-linux-x64"
  $SUDO install -m 0755 "$tmp/tailwindcss" /usr/local/bin/tailwindcss
  rm -rf "$tmp"
fi
tailwindcss --help >/dev/null 2>&1 && echo "tailwindcss ready"

log "Warm the workspace build"
cargo build --locked

log "Environment ready"

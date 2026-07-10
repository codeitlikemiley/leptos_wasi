#!/bin/bash
set -euo pipefail

# Ensure we are in the script's directory (project root)
cd "$(dirname "$0")"

echo "=== Building Guest App for WASIp2 ==="
LEPTOS_OUTPUT_NAME=test-app cargo build --locked --manifest-path tests/test-app/Cargo.toml --target wasm32-wasip2 --release --features islands-router

echo "=== Copying WASIp2 build ==="
cp tests/test-app/target/wasm32-wasip2/release/test_app.wasm tests/test-app-p2.wasm

echo "=== Building Guest App for WASIp3 ==="
LEPTOS_OUTPUT_NAME=test-app cargo build --locked --manifest-path tests/test-app/Cargo.toml --target wasm32-wasip2 --release --no-default-features --features wasip3,islands-router

echo "=== Copying WASIp3 build ==="
cp tests/test-app/target/wasm32-wasip2/release/test_app.wasm tests/test-app-p3.wasm

echo "=== Running Wasmtime E2E tests ==="
cargo test --locked --test e2e test_e2e_wasip -- --ignored --nocapture --test-threads=1

echo "=== Running Spin E2E tests ==="
cargo test --locked --test e2e test_e2e_spin -- --ignored --nocapture --test-threads=1

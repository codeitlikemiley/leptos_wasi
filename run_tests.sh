#!/bin/bash
set -euo pipefail

# Ensure we are in the script's directory (project root)
cd "$(dirname "$0")"

build_component() {
    local feature_flags="$1"
    local output_component="$2"
    
    echo "Building Wasm component directly targeting wasm32-wasip2..."
    LEPTOS_OUTPUT_NAME=test-app cargo build --manifest-path tests/test-app/Cargo.toml --target wasm32-wasip2 --release $feature_flags
    
    local build_wasm="tests/test-app/target/wasm32-wasip2/release/test_app.wasm"
    
    echo "Copying component to $output_component..."
    cp "$build_wasm" "$output_component"
}

echo "=== Building Guest App for WASIp2 ==="
build_component "" "tests/test-app-p2.wasm"

echo "=== Building Guest App for WASIp3 ==="
build_component "--no-default-features --features wasip3" "tests/test-app-p3.wasm"

echo "=== Running Wasmtime E2E tests ==="
cargo test --test e2e test_e2e_wasip -- --ignored --nocapture

echo "=== Running Spin E2E tests ==="
cargo test --test e2e test_e2e_spin -- --ignored --nocapture

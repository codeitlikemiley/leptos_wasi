#!/bin/bash
set -euo pipefail

# Ensure we are in the script's directory (project root)
cd "$(dirname "$0")"

# Locate the WASI Preview 1 adapter dynamically
ADAPTER=$(find /Users/uriah/.cargo -name "wasi_snapshot_preview1.reactor.wasm" | head -n 1)
if [ -z "$ADAPTER" ] || [ ! -f "$ADAPTER" ]; then
    echo "Error: Could not locate wasi_snapshot_preview1.reactor.wasm in ~/.cargo"
    exit 1
fi
echo "Using WASI adapter: $ADAPTER"

build_component() {
    local feature_flags="$1"
    local output_component="$2"
    
    echo "Building core Wasm module..."
    LEPTOS_OUTPUT_NAME=test-app cargo build --manifest-path tests/test-app/Cargo.toml --target wasm32-wasip1 --release $feature_flags
    
    local build_dir="tests/test-app/target/wasm32-wasip1/release"
    
    echo "Decompiling to WAT..."
    wasm-tools print "$build_dir/test_app.wasm" > "$build_dir/test_app.wat"
    
    echo "Stubbing out browser imports..."
    python3 stub_placeholder.py "$build_dir/test_app.wat" "$build_dir/test_app_stubbed.wat"
    
    echo "Recompiling to Wasm..."
    wasm-tools parse "$build_dir/test_app_stubbed.wat" -o "$build_dir/test_app_stubbed.wasm"
    
    echo "Packaging WASIp2 Component..."
    wasm-tools component new "$build_dir/test_app_stubbed.wasm" --adapt "$ADAPTER" -o "$output_component"
}

echo "=== Building Guest App for WASIp2 ==="
build_component "" "tests/test-app-p2.wasm"

echo "=== Building Guest App for WASIp3 ==="
build_component "--no-default-features --features wasip3" "tests/test-app-p3.wasm"

echo "=== Running Wasmtime E2E tests ==="
cargo test --test e2e test_e2e_wasip -- --ignored --nocapture

echo "=== Running Spin E2E tests ==="
cargo test --test e2e test_e2e_spin -- --ignored --nocapture

#!/bin/bash
set -euo pipefail

# This script builds a Leptos WASI example app, stubs out browser imports,
# and packages it as a WASIp2 component.

if [ "$#" -lt 2 ]; then
    echo "Usage: $0 <example-name> <runtime-type>"
    echo "Example: $0 counter wasmtime"
    echo "Example: $0 spin_counter spin"
    exit 1
fi

EXAMPLE_NAME="$1"
RUNTIME_TYPE="$2"

# 1. Locate the WASI Preview 1 adapter dynamically
ADAPTER=$(find /Users/uriah/.cargo -name "wasi_snapshot_preview1.reactor.wasm" | head -n 1)
if [ -z "$ADAPTER" ] || [ ! -f "$ADAPTER" ]; then
    echo "Error: Could not locate wasi_snapshot_preview1.reactor.wasm in ~/.cargo"
    exit 1
fi

echo "Building client assets and server for $EXAMPLE_NAME ($RUNTIME_TYPE)..."

if [ "$RUNTIME_TYPE" = "wasmtime" ]; then
    # Counter example for wasmtime uses cargo leptos build --release
    WASI_RUNTIME=wasmtime cargo leptos build --release --frontend-only
    WASI_RUNTIME=wasmtime LEPTOS_OUTPUT_NAME=counter cargo build --lib --target wasm32-wasip1 --release --no-default-features --features ssr --target-dir target/wasmtime
    
    # Paths
    CORE_WASM="target/wasmtime/wasm32-wasip1/release/counter.wasm"
    WAT_FILE="target/wasmtime/wasm32-wasip1/release/counter.wat"
    STUBBED_WAT="target/wasmtime/wasm32-wasip1/release/counter_stubbed.wat"
    STUBBED_WASM="target/wasmtime/wasm32-wasip1/release/counter_stubbed.wasm"
    OUTPUT_WASM="target/wasmtime/wasm32-wasip2/release/counter.wasm"
    
elif [ "$EXAMPLE_NAME" = "spin-counter" ] || [ "$EXAMPLE_NAME" = "spin_counter" ]; then
    # Spin-counter build
    cargo leptos build --release --frontend-only
    LEPTOS_OUTPUT_NAME=spin_counter cargo build --lib --target wasm32-wasip1 --release --no-default-features --features ssr
    
    # Paths
    CORE_WASM="target/wasm32-wasip1/release/spin_counter.wasm"
    WAT_FILE="target/wasm32-wasip1/release/spin_counter.wat"
    STUBBED_WAT="target/wasm32-wasip1/release/spin_counter_stubbed.wat"
    STUBBED_WASM="target/wasm32-wasip1/release/spin_counter_stubbed.wasm"
    OUTPUT_WASM="target/wasm32-wasip2/release/spin_counter.wasm"
    
else
    # Counter example for Spin build
    WASI_RUNTIME=spin cargo leptos build --release --frontend-only
    WASI_RUNTIME=spin LEPTOS_OUTPUT_NAME=counter cargo build --lib --target wasm32-wasip1 --release --no-default-features --features ssr
    
    # Paths
    CORE_WASM="target/wasm32-wasip1/release/counter.wasm"
    WAT_FILE="target/wasm32-wasip1/release/counter.wat"
    STUBBED_WAT="target/wasm32-wasip1/release/counter_stubbed.wat"
    STUBBED_WASM="target/wasm32-wasip1/release/counter_stubbed.wasm"
    OUTPUT_WASM="target/wasm32-wasip2/release/counter.wasm"
fi

# 2. Decompile to WAT
echo "Decompiling to WAT..."
wasm-tools print "$CORE_WASM" > "$WAT_FILE"

# 3. Stub out browser imports using python script
echo "Stubbing out browser imports..."
# The script stub_placeholder.py is in the repo root
python3 ../../stub_placeholder.py "$WAT_FILE" "$STUBBED_WAT"

# 4. Recompile to WASM
echo "Recompiling to WASM..."
wasm-tools parse "$STUBBED_WAT" -o "$STUBBED_WASM"

# 5. Package as WASIp2 Component
echo "Packaging as WASIp2 Component..."
mkdir -p "$(dirname "$OUTPUT_WASM")"
wasm-tools component new "$STUBBED_WASM" --adapt "$ADAPTER" -o "$OUTPUT_WASM"

echo "Successfully built and componentized: $OUTPUT_WASM"

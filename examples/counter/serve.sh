#!/bin/bash

# Build the project first
echo "Building the project..."
cargo leptos build --release

# Check if build was successful
if [ $? -ne 0 ]; then
    echo "Build failed. Please fix the errors and try again."
    exit 1
fi

# Run the WASM component with wasmtime
echo "Starting the server..."
echo "The server will be available at http://localhost:8080"
echo ""
echo "Note: The --dir=target/site::/ flag mounts the target/site directory"
echo "This allows the WASM component to serve the static assets"
echo ""

wasmtime serve target/server/wasm32-wasip2/wasm-release/counter.wasm \
    -S cli \
    --addr 0.0.0.0:8080 \
    --dir=target/site::/ \
    --env LEPTOS_SITE_ROOT=target/site \
    --env LEPTOS_SITE_PKG_DIR=public/pkg
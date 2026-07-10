# Makefile for building counter app for different WASI runtimes
.PHONY: help spin wasmtime middleware-spin middleware-wasmtime verify-split clean run

# Default target
help:
	@echo "Available targets:"
	@echo "  make spin     - Build and confirm the Spin final-WASI canary"
	@echo "  make wasmtime - Build and run with Wasmtime"
	@echo "  make middleware-spin - Confirm the composed Spin compatibility canary"
	@echo "  make middleware-wasmtime - Run a precomposed middleware component"
	@echo "  make verify-split - Verify lazy WASM split artifacts"
	@echo "  make clean    - Clean build artifacts"

# Run target alias
run: wasmtime

# Build and confirm Spin's expected final-WASI linker rejection
spin:
	@echo "Building the final-WASI component for the Spin compatibility canary..."
	@mkdir -p .spin
	cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
	LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib --target wasm32-wasip2 --release --no-default-features --features ssr
	@$(MAKE) verify-split
	@echo "Built: target/wasm32-wasip2/release/counter.wasm"
	PORT=3000 ../../scripts/check-spin-final-wasi-canary.sh "$(CURDIR)/spin.toml"

# Build and run with Wasmtime
wasmtime:
	@echo "Building for Wasmtime runtime..."
	cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
	LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib --target wasm32-wasip2 --release --no-default-features --features ssr --target-dir target/wasmtime
	@$(MAKE) verify-split
	@echo "Built: target/wasmtime/wasm32-wasip2/release/counter.wasm (for Wasmtime)"
	@echo "Starting Wasmtime serve..."
	@echo "Serving at http://127.0.0.1:3000"
	wasmtime serve \
		-W component-model-async=y \
		-S p3=y \
		-S cli=y \
		-S http=y \
		--dir=$(CURDIR)/target/site::/site \
		--env=LEPTOS_OUTPUT_NAME=counter \
		--env=LEPTOS_SITE_ROOT=/site \
		--env=LEPTOS_SITE_PKG_DIR=pkg \
		--addr 127.0.0.1:3000 \
		target/wasmtime/wasm32-wasip2/release/counter.wasm

middleware-spin:
	@echo "Confirming Spin 4's expected final-WASI linker rejection..."
	MIDDLEWARE=1 HOST=spin ../../tests/browser/run.sh

middleware-wasmtime:
	@echo "Running the final-WASI composed middleware example on Wasmtime..."
	MIDDLEWARE=1 HOST=wasmtime ../../tests/browser/run.sh

verify-split:
	@test -f target/site/pkg/counter.js
	@test -f target/site/pkg/counter.wasm
	@test -f target/site/pkg/__wasm_split_manifest.json
	@test -f target/site/pkg/__wasm_split.______________________.js
	@test -n "$$(find target/site/pkg -maxdepth 1 -name 'split_*.wasm' -print -quit)"
	@echo "Verified islands entrypoint and lazy WASM split artifacts in target/site/pkg."

# Clean target
clean:
	cargo clean
	rm -rf target/site target/wasmtime
	rm -rf .spin

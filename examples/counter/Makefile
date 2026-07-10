# Makefile for building counter app for different WASI runtimes
.PHONY: help spin wasmtime middleware-spin middleware-wasmtime verify-split clean run

# Default target
help:
	@echo "Available targets:"
	@echo "  make spin     - Build and run with Spin"
	@echo "  make wasmtime - Build and run with Wasmtime"
	@echo "  make middleware-spin - Run with pinned experimental Spin middleware"
	@echo "  make middleware-wasmtime - Run a precomposed middleware component"
	@echo "  make verify-split - Verify lazy WASM split artifacts"
	@echo "  make clean    - Clean build artifacts"

# Run target alias
run: wasmtime

# Build and run with Spin
spin:
	@echo "Building for Spin runtime..."
	@mkdir -p .spin
	cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
	LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib --target wasm32-wasip2 --release --no-default-features --features ssr
	@$(MAKE) verify-split
	@echo "Built: target/wasm32-wasip2/release/counter.wasm (for Spin)"
	@echo "Starting Spin..."
	spin up

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
	@echo "Building the experimental Spin middleware example..."
	cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
	LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib --target wasm32-wasip2 --release --no-default-features --features ssr
	@$(MAKE) verify-split
	../../scripts/audit-middleware-manifests.py --spin
	../../scripts/build-middleware-fixture.sh
	spin up --file spin.middleware-vnext.toml

middleware-wasmtime:
	@echo "Building the experimental Wasmtime middleware example..."
	cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
	LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib --target wasm32-wasip2 --release --no-default-features --features ssr
	@$(MAKE) verify-split
	../../scripts/audit-middleware-manifests.py --wasmtime --composition-tools
	../../scripts/build-middleware-fixture.sh
	../../scripts/compose-middleware.sh \
		target/wasm32-wasip2/release/counter.wasm \
		target/wasm32-wasip2/release/counter-middleware.wasm \
		../../tests/middleware-fixture.wasm
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
		target/wasm32-wasip2/release/counter-middleware.wasm

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

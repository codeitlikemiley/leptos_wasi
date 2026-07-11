# Makefile for building counter app for different WASI runtimes
.PHONY: help bootstrap-spin bootstrap-spin-no-default spin spin-stable-canary wasmtime trusted-ingress-spin middleware-spin middleware-wasmtime verify-split clean run

# Default target
help:
	@echo "Available targets:"
	@echo "  make bootstrap-spin - Build the exact pinned Spin main runtime"
	@echo "  make spin     - Build and run on the exact pinned Spin main runtime"
	@echo "  make spin-stable-canary - Confirm Spin 4.0.2 cannot host final WASI"
	@echo "  make trusted-ingress-spin - Run authn/Cedar/SpiceDB through pinned Spin main"
	@echo "  make wasmtime - Build and run with Wasmtime"
	@echo "  make middleware-spin - Run the explicit no-CPU-metrics Spin diagnostic"
	@echo "  make middleware-wasmtime - Run a precomposed middleware component"
	@echo "  make verify-split - Verify lazy WASM split artifacts"
	@echo "  make clean    - Clean build artifacts"

# Run target alias
run: wasmtime

bootstrap-spin:
	../../scripts/bootstrap-spin-main.sh

bootstrap-spin-no-default:
	../../scripts/bootstrap-spin-main.sh no-default-features

# Build and run on the exact tested Spin main revision.
spin:
	@echo "Building the final-WASI component for pinned Spin main..."
	@mkdir -p .spin
	cargo leptos build --release --split --frontend-only --lib-cargo-args=--locked
	LEPTOS_OUTPUT_NAME=counter cargo build --locked --lib --target wasm32-wasip2 --release --no-default-features --features ssr
	@$(MAKE) verify-split
	@echo "Built: target/wasm32-wasip2/release/counter.wasm"
	PORT=3000 ../../scripts/run-spin-main.sh "$(CURDIR)/spin.toml"

spin-stable-canary:
	PORT=3000 ../../scripts/check-spin-final-wasi-canary.sh "$(CURDIR)/spin.toml"

trusted-ingress-spin:
	@binary="$$(../../scripts/bootstrap-spin-main.sh)"; \
	SPIN_BIN="$$binary" ../../scripts/run-trusted-ingress-spin-main.sh

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
	@echo "Running the non-promoting Spin main middleware diagnostic..."
	@binary="$$(../../scripts/bootstrap-spin-main.sh no-default-features)"; \
	SPIN_BIN="$$binary" SPIN_DIAGNOSTIC_NO_DEFAULT_FEATURES=1 PORT=3110 MIDDLEWARE=1 HOST=spin ../../tests/browser/run.sh

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

# Apples-to-apples: same Leptos app on leptos_wasi (Wasmtime P2) vs axum.
.PHONY: help build build-wasi build-axum wasi axum compare clean

WASI_PORT ?= 3000
AXUM_PORT ?= 3001
DURATION ?= 10
CONCURRENCY ?= 20
WARMUP ?= 25
REUSE_COUNT ?= 128
WASI_WASM := target/wasm32-wasip2/release/compare_wasi.wasm
AXUM_BIN := target/release/compare-axum

help:
	@echo "Available targets:"
	@echo "  make build      - Compile both backends (release)"
	@echo "  make wasi       - Build and serve WASI Preview 2 on :$(WASI_PORT)"
	@echo "  make axum       - Build and serve native axum on :$(AXUM_PORT)"
	@echo "  make compare    - Build, start both, warm, print rps/latency table"
	@echo "  make clean      - cargo clean"

build: build-wasi build-axum

build-wasi:
	cargo build --locked -p compare-wasi --target wasm32-wasip2 --release

build-axum:
	cargo build --locked -p compare-axum --release

wasi: build-wasi
	@echo "WASI Preview 2 on http://127.0.0.1:$(WASI_PORT) (reuse $(REUSE_COUNT))"
	wasmtime serve \
		-S cli=y \
		--max-instance-reuse-count $(REUSE_COUNT) \
		--dir "$(CURDIR)/public::/static" \
		--addr "127.0.0.1:$(WASI_PORT)" \
		"$(CURDIR)/$(WASI_WASM)"

axum: build-axum
	@echo "axum on http://127.0.0.1:$(AXUM_PORT)"
	LEPTOS_SITE_ADDR=127.0.0.1:$(AXUM_PORT) "$(CURDIR)/$(AXUM_BIN)"

compare: build
	WASI_PORT=$(WASI_PORT) AXUM_PORT=$(AXUM_PORT) \
	DURATION=$(DURATION) CONCURRENCY=$(CONCURRENCY) WARMUP=$(WARMUP) \
	WASMTIME_MAX_INSTANCE_REUSE_COUNT=$(REUSE_COUNT) \
	SKIP_BUILD=1 \
	./scripts/compare.sh

clean:
	cargo clean

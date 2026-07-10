# counter

This example demonstrates running a Leptos application utilizing the native WASI Preview 3 (WASIp3) task scheduling and standard HTTP triggers. It supports both raw Wasmtime and Spin as runtimes.

## Prerequisites

- **Rust Toolchain:** Version 1.93.0 or later (required by `spin-sdk` v6.0.0).
- **Rust target:** `rustup target add wasm32-wasip2`
- **Cargo Leptos:** Version 0.3.7 or later (`cargo install --locked cargo-leptos`).
- **Spin CLI:** Version 4.0.0 or later.
- **Wasmtime CLI:** Version 45.0.0 or later.

## Build and Run

To compile and run the application under Wasmtime:

```bash
make wasmtime
```

To compile and run the application under Spin:

```bash
make spin
```

To clean up all local build and storage files:

```bash
make clean
```

Once running, access the application at `http://127.0.0.1:3000`.

Both commands build the frontend with `cargo leptos --split`. You can verify
the generated main module, lazy island chunk, loader, and manifest without
starting a runtime:

```bash
make verify-split
```

The interactive counter uses `#[island(lazy)]`. The router, page layout,
headings, and explanatory content are server-only components. The browser first
loads the small islands runtime and then loads the counter island from its own
`split_*.wasm` file.

This first WASI rollout intentionally keeps Cargo Leptos file hashing disabled.
The Spin file server uses `Cache-Control: no-cache`, and the whole `pkg`
directory should be deployed atomically so the loader, manifest, main module,
and lazy chunk always describe the same build.

## Architecture & Storage Backend

1. **Storage:** The storage mechanism depends on the runtime:
   - **Spin:** Persists the count using Spin's built-in key-value store (configured as the `"default"` store in `spin.toml`).
   - **Wasmtime:** Persists the count to `/data/counter.txt` inside the component's sandboxed filesystem, mapped via `--dir=./data::/data` to a local directory on your host.
2. **Static Files:** Wasmtime maps `./target/site/` to guest `/site`, while Spin serves `./target/site/pkg/` from a dedicated `/pkg/...` file-server component. Both serve every generated split asset.
3. **Split Manifest:** The server component can read `/site/pkg/__wasm_split_manifest.json`, allowing Leptos SSR to emit preload hints for server-invoked lazy functions and routes. The lazy island itself loads when island hydration runs.
4. **WASI HTTP:** The server implements `wasip3::exports::http::handler::Guest` and runs as a native WebAssembly component using the Preview 3 async ABI.

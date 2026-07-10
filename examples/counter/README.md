# counter

This example demonstrates running a Leptos application using native WASI
Preview 3 task scheduling and standard HTTP triggers. Wasmtime 46 is the
blocking final-WASI runtime. The Spin manifests are retained as compatibility
canaries until a tagged Spin release links final `wasi:http@0.3.0`.

## Prerequisites

- **Rust Toolchain:** Version 1.93.0 or later.
- **Rust target:** `rustup target add wasm32-wasip2`
- **Cargo Leptos:** Version 0.3.7 or later (`cargo install --locked cargo-leptos`).
- **Spin CLI:** Version 4.0.0 or later.
- **Wasmtime CLI:** Version 46.0.1 or later.

## Build and Run

To compile and run the application under Wasmtime:

```bash
make wasmtime
```

To compile the application and confirm Spin's pinned expected linker failure:

```bash
make spin
```

To clean up all local build files:

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

## Architecture

1. **Session Counter:** Each hydrated island starts at zero. The browser submits its current value to a server function, which performs a checked increment and returns the next value. No count is persisted or shared between browser sessions.
2. **Static Files:** Wasmtime maps `./target/site/` to guest `/site`, while Spin serves `./target/site/pkg/` from a dedicated `/pkg/...` file-server component. Both serve every generated split asset.
3. **Split Manifest:** The server component can read `/site/pkg/__wasm_split_manifest.json`, allowing Leptos SSR to emit preload hints for server-invoked lazy functions and routes. The lazy island itself loads when island hydration runs.
4. **WASI HTTP:** The server implements `wasip3::exports::http::handler::Guest` and runs as a native WebAssembly component using the Preview 3 async ABI.

## Component middleware

The local middleware runner verifies the sibling `wasi-http-middleware`
artifacts and deterministically composes request ID, security headers, CORS,
and optional authentication around the counter. Wasmtime runs the precomposed
final `wasi:http@0.3.0` component. Stable Spin 4 cannot link the final HTTP
resource types, while `spin.middleware-vnext.toml` is a second incompatibility
canary because Spin's native middleware implementation still hard-codes the
March RC handler world.

The `/pkg/...` file-server trigger deliberately has no authentication
middleware. It must remain public so the browser can load `counter.js`,
`counter.wasm`, and lazy `split_*.wasm` chunks. Middleware attached to the
`counter` trigger does not apply to this separate asset trigger.

Run the browser-verified example on Wasmtime, or confirm the Spin canary, with:

```bash
make middleware-wasmtime
# or
make middleware-spin
```

The exact SDK, Spin runtime, WIT, Wasmtime, and `wac` versions are recorded in
`../../tests/middleware/components.lock.toml`. Spin is promoted only after a
tagged release provides final handler, types, and client host support and
native middleware composition against the final WIT.

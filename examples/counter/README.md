# counter

This example demonstrates running a Leptos application utilizing the native WASI Preview 3 (WASIp3) task scheduling and standard HTTP triggers. It supports both raw Wasmtime and Spin as runtimes.

## Prerequisites

- **Rust Toolchain:** Version 1.93.0 or later.
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

## Experimental component middleware

`spin.middleware-vnext.toml` demonstrates Spin's vNext
`dependencies.middleware` composition using the protocol-only fixture from
this repository. The middleware is composed in-process around the `counter`
service; it is not a separately deployed proxy and it does not change the
`leptos_wasi::Handler` API.

The `/pkg/...` file-server trigger deliberately has no authentication
middleware. It must remain public so the browser can load `counter.js`,
`counter.wasm`, and lazy `split_*.wasm` chunks. Middleware attached to the
`counter` trigger does not apply to this separate asset trigger.

Run the example against the pinned experimental toolchain with:

```bash
make middleware-wasmtime
# or
make middleware-spin
```

The exact SDK, Spin runtime, WIT, Wasmtime, and `wac` versions are recorded in
`../../tests/middleware/components.lock.toml`. This remains experimental until
Spin publishes stable middleware composition using a WIT revision compatible
with the application.

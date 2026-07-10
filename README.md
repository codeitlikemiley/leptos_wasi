<div align="center">
  <h1><code>leptos_wasi</code></h1>

  <p>
    <strong>Run your Leptos Server-Side in
    <a href="https://webassembly.org/">WebAssembly</a>
    using WASI standards.
    </strong>
  </p>
</div>

## Overview

[Leptos](https://leptos.dev) gives you tools to build web applications with
best-in-class performance using WebAssembly in the browser.

This crate takes it further — it lets you also run your
[Leptos Server](https://book.leptos.dev/server/index.html) as a
[WebAssembly Component][wasm-component] targeting the
[`wasi:http`](https://github.com/WebAssembly/wasi-http) proposal. Deploy your
server on any WASI-compatible runtime:

- **[Wasmtime](https://wasmtime.dev)** — the reference runtime from the
  [Bytecode Alliance](https://bytecodealliance.org/)
- **[Spin](https://developer.fermyon.com/spin/v3)** — Fermyon's serverless
  application platform

### Demo

https://github.com/user-attachments/assets/6596e0f3-80c0-4258-a4e3-f85c41b328b4

## Prerequisites

- **Rust:** ≥ 1.93.0 (required by `spin-sdk` v6.0.0)
- **Target:** `rustup target add wasm32-wasip2`
- **Cargo Leptos:** ≥ 0.3.7 (`cargo install --locked cargo-leptos`)
- **Wasmtime:** ≥ 45.0.0 *(if serving under Wasmtime)*
- **Spin:** ≥ 4.0.0 *(if serving under Spin)*

## Compatibility

| Dependency | Version | Notes |
|------------|---------|-------|
| Leptos | `0.8.20` | Fully tested |
| Spin SDK | `6.0.0` | WASIp3 native HTTP triggers |
| `wasi` | `0.14.7` | WASIp2 types and polling interfaces |
| `wasip3` | `0.6.0` | WASIp3 core types, host spawner, HTTP compatibility |
| `http-body` | `1.0.0` | Standard streaming response frames |

## Feature Flags

Choose one dependency declaration for the server runtime.

WASI Preview 2 (default cooperative polling executor):

```toml
[dependencies]
leptos_wasi = "0.3.2"
```

WASI Preview 3 (native host-level task spawning):

```toml
[dependencies]
leptos_wasi = { version = "0.3.2", default-features = false, features = ["wasip3"] }
```

WASI Preview 3 plus the server half of optional islands-router navigation:

```toml
[dependencies]
leptos_wasi = { version = "0.3.2", default-features = false, features = ["wasip3", "islands-router"] }
```

> [!NOTE]
> Both features can be enabled simultaneously (`--all-features`). When both
> `wasip2` and `wasip3` are active, the `wasip3` pipeline takes precedence.

## Islands and Lazy Browser WASM

Leptos islands and browser WASM splitting work with the same `Handler` on
Wasmtime and Spin. They split the browser target (`wasm32-unknown-unknown`),
not the WASI server component.

Enable islands in both client and server feature sets:

```toml
[features]
hydrate = ["leptos/hydrate", "leptos/islands"]
ssr = ["leptos/ssr", "leptos/islands", "dep:leptos_wasi"]
```

Use the islands browser entrypoint and shell script:

```rust
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    leptos::mount::hydrate_islands();
}

// In the server-rendered document shell:
view! { <HydrationScripts options islands=true /> }
```

Keep layouts and pages as server components, then opt only interactive widgets
into browser code. Adding `lazy` gives that island its own WASM chunk:

```rust
#[island(lazy)]
fn InteractiveWidget() -> impl IntoView {
    // Browser-side signals, events, and server-function calls.
}
```

Build the browser assets with splitting enabled, then build the WASI server as
usual:

```bash
cargo leptos build --release --split --frontend-only
LEPTOS_OUTPUT_NAME=my_app cargo build --lib --target wasm32-wasip2 \
  --release --no-default-features --features ssr
```

Deploy the entire `target/site/pkg` directory. It contains the main browser
module, split chunks, loader JavaScript, and split manifest. The asset server
must return `.wasm` files as `application/wasm`; `Handler::static_files_handler`
does this automatically. Mount `target/site` into the server guest at the path
described by `LEPTOS_SITE_ROOT` so Leptos can read the split manifest and emit
preload hints.

> [!WARNING]
> Keep Cargo Leptos `hash-files` disabled for WASI in this initial rollout.
> Leptos currently locates the hash manifest through `current_exe()`, which is
> unsupported by Rust's WASI standard library. Enabling hash mode without also
> packaging `hash.txt` at an explicit guest path can panic during SSR. Deploy
> the unhashed package atomically and disable long-lived asset caching instead.

The optional `islands-router` feature adds request-aware rendering for Leptos's
client-side islands navigation. The handler detects the `Islands-Router`
request header, provides `IslandsRouterNavigation`, and switches navigation
responses to in-order branching. Enable the matching Leptos feature in both app
targets and install the browser routing script in the document shell:

```toml
[features]
hydrate = [
  "leptos/hydrate",
  "leptos/islands",
  "leptos/islands-router",
]
ssr = [
  "leptos/ssr",
  "leptos/islands",
  "leptos/islands-router",
  "dep:leptos_wasi",
  "leptos_wasi/islands-router",
]
```

```rust
view! {
    <HydrationScripts
        options
        islands=true
        islands_router=true
    />
}
```

Ordinary multi-page islands do not need this feature.

## Quick Start

### WASIp3 with Wasmtime

```rust
use leptos::config::get_configuration;
use leptos_wasi::executor::init_wasip3_spawner;
use leptos_wasi::prelude::Handler;
use wasip3::http::types::{Request, Response, ErrorCode};

use crate::app::{shell, App, GetCount, IncrementCount};

struct LeptosServer;

impl wasip3::exports::http::handler::Guest for LeptosServer {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let _ = init_wasip3_spawner();

        let conf = get_configuration(None).unwrap();
        let leptos_options = conf.leptos_options;

        let req = wasip3::http_compat::http_from_wasi_request(request)?;

        let wasi_res = Handler::build(req).await
            .map_err(|e| {
                eprintln!("Error building handler: {:?}", e);
                ErrorCode::InternalError(None)
            })?
            .with_server_fn::<GetCount>()
            .with_server_fn::<IncrementCount>()
            .generate_routes(App)
            .handle_with_context(move || shell(leptos_options.clone()), || {})
            .await
            .map_err(|e| {
                eprintln!("Error handling request: {:?}", e);
                ErrorCode::InternalError(None)
            })?;

        Ok(wasi_res)
    }
}

wasip3::http::service::export!(LeptosServer);
```

```bash
cargo leptos build --release
wasmtime serve \
    -W component-model-async=y \
    -S p3=y -S cli=y -S http=y \
    target/server/wasm32-wasip2/release/your_crate.wasm
```

### WASIp3 with Spin

```rust
use leptos_wasi::executor::init_wasip3_spawner;
use leptos_wasi::prelude::Handler;
use spin_sdk::{http::{IntoResponse, Request, Response}, http_service};

#[http_service]
async fn handle_request(req: Request) -> Result<impl IntoResponse, anyhow::Error> {
    let _ = init_wasip3_spawner();
    // ... build Handler and serve
}
```

```bash
spin build --up
```

### WASIp2 (Cooperative Polling)

```rust
use any_spawner::Executor as LeptosExecutor;
use leptos_wasi::prelude::{IncomingRequest, ResponseOutparam, WasiExecutor};
use wasi::exports::http::incoming_handler::Guest;

struct LeptosServer;

impl Guest for LeptosServer {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let executor = WasiExecutor::new(leptos_wasi::executor::Mode::Stalled);
        LeptosExecutor::init_local_custom_executor(executor.clone()).unwrap();

        executor.run_until(async {
            Handler::build(request, response_out).unwrap()
                .with_server_fn::<GetCount>()
                .generate_routes(App)
                .handle_with_context(move || shell(leptos_options.clone()), || {})
                .await
                .unwrap();
        })
    }
}

wasi::http::proxy::export!(LeptosServer with_types_in wasi);
```

```bash
cargo leptos build --release
wasmtime serve target/server/wasm32-wasip2/release/your_crate.wasm -Scommon
```

## Server Function Registration

```rust
// Register a server function (body types inferred automatically):
.with_server_fn::<MyServerFn>()
```

## Static File Serving

```rust
fn serve_static_files(path: String) -> Option<leptos_wasi::response::Body> {
    // Return None for 404, Some(body) for file content
}

handler.static_files_handler("/pkg", serve_static_files)
```

## Examples

| Example | Runtime | Description |
|---------|---------|-------------|
| [counter](./examples/counter) | Wasmtime + Spin | Dual-runtime counter with compile-time storage backend switching (`build.rs`) |
| [spin-counter](./examples/spin-counter) | Spin only | Counter using Spin's built-in key-value store |

## Core Features

* :rocket: **Dual Async Runtimes**:
  - **WASIp2**: Single-threaded cooperative polling executor using [`pollable`][wasip2-pollable] for non-blocking I/O and Leptos streaming [SSR Modes][leptos-ssr-modes].
  - **WASIp3**: Native host-level task spawning via `wasip3::wit_bindgen::spawn` — zero guest-side event loop overhead.
* :zap: **Short-circuiting**: Avoids rendering work entirely when serving static files or server functions.
* :truck: **Custom Static Asset Serving**: Plug your own serving logic (e.g., [`wasi:blobstore`][wasi-blobstore] for object storage).
* :desert_island: **Islands and Split WASM**: Server-only page shells, lazy browser islands, and request-aware islands-router rendering across Wasmtime and Spin.
* :gear: **Multiple Server Backends**: Axum, generic, and custom server function backends.

## Troubleshooting

**Server function not found (404):**
Ensure every `#[server]` function is registered on the handler:
```rust
.with_server_fn::<YourServerFn>()
```

[leptos-ssr-modes]: https://book.leptos.dev/ssr/23_ssr_modes.html
[wasip2-pollable]: https://github.com/WebAssembly/wasi-io/blob/main/wit/poll.wit
[wasi-blobstore]: https://github.com/WebAssembly/wasi-blobstore
[wasm-component]: https://component-model.bytecodealliance.org

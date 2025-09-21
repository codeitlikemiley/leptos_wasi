<div align="center">
  <h1><code>leptos_wasi</code></h1>

  <p>
    <strong>Run your Leptos Server-Side in
    <a href="https://webassembly.org/">WebAssembly</a>
    using WASI standards.
    </strong>
  </p>
</div>

## Explainer

WebAssembly is already popular in the browser but organisations like the
[Bytecode Alliance][bc-a] are committed to providing the industry with new
standard-driven ways of running software. Specifically, they are maintaining
the [Wasmtime][wasmtime] runtime, which allows running WebAssembly out of the
browser (e.g., on a serverless platform).

Leptos is already leveraging WebAssembly in the browser and gives you tools to
build web applications with best-in-class performance.

This crate aims to go further and enable you to also leverage WebAssembly for
your [Leptos Server][leptos-server]. Specifically, it will allow you to
target the rust `wasm32-wasip2` target for the server-side while integrating
seamlessly with the Leptos Framework.

Running `cargo leptos build` will provide you with a
[WebAssembly Component][wasm-component] importing the
[`wasi:http/proxy` world][wasi-http-proxy]. This means you can serve
your server on any runtime supporting this world, for example:

```shell
wasmtime serve target/server/wasm32-wasip2/debug/your_crate.wasm -Scommon
```

[bc-a]: https://bytecodealliance.org/
[leptos-server]: https://book.leptos.dev/server/index.html
[wasmtime]: https://wasmtime.dev
[wasi-http-proxy]: https://github.com/WebAssembly/wasi-http/blob/main/proxy.md
[wasm-component]: https://component-model.bytecodealliance.org

## Disclaimer

This crate is **EXPERIMENTAL** and the author is not affiliated with the Bytecode
Alliance nor funded by any organisation. Consider this crate should become a
community-driven project and be battle-tested to be deemed *production-ready*.

Contributions are welcome!

## Usage

### Installation

Add `leptos_wasi` to your `Cargo.toml` dependencies. The crate supports two modes depending on your deployment target:

#### For Standard Leptos Projects

For standard Leptos server-side applications (non-Spin), use the default features:

```toml
[dependencies]
leptos_wasi = { version = "0.1.4" }
# or explicitly:
leptos_wasi = { version = "0.1.4", features = ["generic"] }
```

#### For Spin WebAssembly Projects

For projects targeting the [Spin](https://www.fermyon.com/spin) WebAssembly runtime:

```toml
[dependencies]
leptos_wasi = { version = "0.1.4", default-features = false, features = ["spin"] }
```

### Feature Flags

- **`generic`** (default): Enables support for standard Leptos server-side applications with full generic body types
- **`spin`**: Enables support for Spin WebAssembly runtime with simplified body types (`Bytes`)
- **`islands-router`**: Enables islands architecture support for partial hydration

### Example Usage

```rust
use leptos_wasi::Handler;

// Your Leptos app component
#[component]
fn App() -> impl IntoView {
    // Your app implementation
}

// In your WASI entry point
#[export_name = "wasi:http/incoming-handler@0.2.0#handle"]
pub unsafe extern "C" fn handle(request: IncomingRequest, response_outparam: ResponseOutparam) {
    let handler = Handler::build(request, response_outparam)
        .expect("Failed to build handler");
    
    // For Spin projects with server functions
    #[cfg(feature = "spin")]
    let handler = handler.with_server_fn::<YourServerFn>();
    
    // Generate routes and handle the request
    handler
        .generate_routes(App)
        .handle_with_context(App, || {})
        .await
        .expect("Failed to handle request");
}
```

### Compatibility

This crate works with Leptos v0.8+ and supports both standard server deployments and WebAssembly Component Model deployments.

## Features

* :octopus: **Async Runtime**: This crate comes with a single-threaded *async* executor
  making full use of WASIp2 [`pollable`][wasip2-pollable], so your server is not
  blocking on I/O and can benefit from Leptos' streaming [SSR Modes][leptos-ssr-modes].
* :zap: **Short-circuiting Mechanism**: Your component is smart enough to avoid
  preparing or doing any *rendering* work if the request routes to static files or
  *Server Functions*.
* :truck: **Custom Static Assets Serving**: You can write your own logic
  for serving static assets. For example, once
  [`wasi:blobstore`][wasi-blobstore] matures up, you could host your static assets
  on your favorite *Object Storage* provider and make your server fetch them
  seamlessly.

[leptos-ssr-modes]: https://book.leptos.dev/ssr/23_ssr_modes.html
[wasip2-pollable]: https://github.com/WebAssembly/wasi-io/blob/main/wit/poll.wit
[wasi-blobstore]: https://github.com/WebAssembly/wasi-blobstore

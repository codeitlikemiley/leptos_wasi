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

### Quick Start

```rust
use leptos_wasi::prelude::*;

// Server function example
#[server(UpdateCount, "/api")]
pub async fn update_count() -> Result<i32, ServerFnError> {
    // Your server logic here
    Ok(42)
}

// Handler setup
Handler::build(request, response_out)?
    .static_files_handler("/public", serve_static_files)
    .with_server_fn_axum::<UpdateCount>()  // Clean syntax!
    .generate_routes(App)
    .handle_with_context(app_fn, additional_context)
    .await?;
```

### Server Function Registration

This crate provides multiple convenient ways to register server functions:

#### 🎯 **Recommended: Axum Backend (Most Common)**
```rust
.with_server_fn_axum::<MyServerFn>()
```
Perfect for most Leptos projects using the default axum backend.

#### 🔧 **Generic Backend**
```rust
.with_server_fn_generic::<MyServerFn>()
```
For projects using custom server backends.

#### 🛠️ **Advanced: Explicit Control**
```rust
.with_server_fn::<MyServerFn, BodyType>()
```
When you need full control over body types.

### Static File Serving

```rust
fn serve_static_files(path: String) -> Option<leptos_wasi::response::Body> {
    // Your static file serving logic
    // Return None for 404, Some(body) for file content
}

handler.static_files_handler("/public", serve_static_files)
```

## Migration Guide

### Upgrading to v0.1.3+

If you're using the older syntax with type placeholders, you can easily upgrade:

#### Before (v0.1.2 and earlier)
```rust
.with_server_fn::<UpdateCount, _>()    // Required ugly placeholder
.with_server_fn::<GetCount, _>()
```

#### After (v0.1.3+)
```rust
.with_server_fn_axum::<UpdateCount>()  // Clean and clear!
.with_server_fn_axum::<GetCount>()
```

The old syntax still works for backward compatibility, but the new methods provide better ergonomics and clearer intent.

### Static File Handler Updates

The static file handler now expects `leptos_wasi::response::Body` directly:

```rust
// Updated signature
fn serve_static_files(path: String) -> Option<leptos_wasi::response::Body> {
    // Implementation remains the same
}
```

### Compatibility

This crate works with **Leptos v0.8.x** (specifically tested with v0.8.9).

**Migration Note**: This crate has been migrated from Leptos 0.7.x to 0.8.x. The migration includes:
- Updated to use `any_spawner` crate instead of `leptos::tasks::Executor`
- Adapted to new server function API changes
- Updated SSR mode handling for new API requirements

## Recent Improvements (v0.1.3+)

### 🎯 **Enhanced Developer Experience**
* **Clean Server Function Registration**: No more ugly `_` placeholders! Use `.with_server_fn_axum::<MyServerFn>()` instead of `.with_server_fn::<MyServerFn, _>()`
* **Automatic Backend Detection**: Choose the right method for your backend with clear, descriptive names
* **Better Error Messages**: Improved compilation errors with helpful suggestions

### 🔧 **Robust Error Handling**
* **Proper Error Propagation**: Body reading errors are now properly converted and propagated instead of being silently dropped
* **Multiple Backend Support**: Seamless integration with different server function backends (axum, generic)
* **Type-Safe Conversions**: Automatic conversion between different Body types with compile-time safety

### 🛠️ **Technical Improvements**
* **Stream-Based Body Processing**: Efficient async streaming for HTTP body handling
* **Enhanced Type System**: Better trait bounds and type constraints for improved reliability
* **Backward Compatibility**: All existing code continues to work while new conveniences are available

## Core Features

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
* :gear: **Multiple Server Backends**: Works seamlessly with axum, generic, and custom server function backends.

## Troubleshooting

### Common Issues

#### "Body type mismatch" errors
If you encounter body type compilation errors, make sure you're using the correct convenience method:
- For axum backend (most common): `.with_server_fn_axum::<YourServerFn>()`
- For other backends: `.with_server_fn_generic::<YourServerFn>()`
- For custom setups: `.with_server_fn::<YourServerFn, YourBodyType>()`

#### "No method named `with_server_fn_axum`"
Make sure you're using leptos_wasi v0.1.3 or later:
```toml
[dependencies]
leptos_wasi = "0.1.3"
```

#### Server function not found (404)
Ensure your server function is properly registered:
```rust
.with_server_fn_axum::<YourServerFn>()  // Must match your #[server] macro
```

### Build Commands

```bash
# Build for WASI target
cargo build --target wasm32-wasip2

# Run with wasmtime
wasmtime serve target/wasm32-wasip2/release/your_crate.wasm -Scommon

# For leptos projects
cargo leptos build --release
```

[leptos-ssr-modes]: https://book.leptos.dev/ssr/23_ssr_modes.html
[wasip2-pollable]: https://github.com/WebAssembly/wasi-io/blob/main/wit/poll.wit
[wasi-blobstore]: https://github.com/WebAssembly/wasi-blobstore

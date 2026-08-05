# `leptos_wasi` migration guide

## Migrating from 0.4.1 to 0.4.2

The 0.4.2 release candidate gives route-generation context lifecycle-explicit
names. The previous methods remain as forwarding aliases, so the
patch release is source compatible while applications migrate.

| Previous method | Replacement |
|---|---|
| `generate_routes_with_context` | `generate_routes_with_discovery_context` |
| `generate_routes_with_exclusions_and_context` | `generate_routes_with_exclusions_and_discovery_context` |

Discovery context runs only while Leptos discovers the route list. It receives
synthetic request contexts, may be skipped when the route list is cached, and
must never depend on headers, authentication, or other request state.

Install request-dependent context through `handle_with_context`. That closure
runs after standard contexts, including `http::request::Parts`, have been
installed for SSR and server-function requests.

This release does not add an HTTP-layer builder API to `leptos_wasi`. Reusable
whole-service authentication and authorization remain externally composed
WASIp3 components. Applications that want the typed bridge use the separately
versioned `leptos-wasi-authz` crate with the trusted
`x-wasi-auth-context` installed by the composed authentication boundary.
`leptos_wasi` itself does not establish trust in that header; an application
that exposes its terminal component directly must not install the bridge.

The 0.4.2 release candidate supports the native terminal path on the maintained
Spin fork. Portable WAC-composed middleware and an upstream tagged Spin runtime
remain separate promotion gates. See [PRODUCTION.md](./PRODUCTION.md) and
[MIDDLEWARE.md](./MIDDLEWARE.md) before enabling experimental composition.

## Migrating from `leptos_wasi` 0.3 to 0.4

Version 0.4 is a deliberate breaking release. It makes Preview 2 and Preview 3
additive, removes redundant server-function aliases, and makes registration and
executor failures explicit.

## 1. Select a runtime namespace

The root prelude no longer exports a runtime-dependent `Handler`.

```rust
// 0.3: selected indirectly by Cargo feature precedence
use leptos_wasi::prelude::Handler;

// 0.4: select the adapter explicitly
use leptos_wasi::wasip2::prelude::Handler;
// or
use leptos_wasi::wasip3::prelude::Handler;
```

Preview 2 remains the default feature. Preview 3 can be used alone or together
with Preview 2:

```toml
# Preview 2 only
leptos_wasi = { package = "leptos-wasi-runtime", version = "0.4.2" }

# Preview 3 only
leptos_wasi = { package = "leptos-wasi-runtime", version = "0.4.2", default-features = false, features = ["wasip3"] }

# Both public adapters
leptos_wasi = { package = "leptos-wasi-runtime", version = "0.4.2", features = ["wasip3"] }
```

Code that enables neither runtime now receives a compile-time error.

### Other relocated or internalized runtime APIs

The 0.4 namespace split intentionally accounts for the remaining public-path
breaks reported by `cargo-semver-checks`:

| 0.3 path | 0.4 replacement |
|---|---|
| `handler::Handler` | `wasip2::Handler` or `wasip3::Handler` |
| `handler::HandlerError` | the selected runtime's `HandlerError` |
| `prelude::WasiExecutor` | `wasip2::prelude::WasiExecutor` |
| `prelude::IncomingRequest` | the selected runtime's `prelude::IncomingRequest` |
| `prelude::ResponseOutparam` | `wasip2::prelude::ResponseOutparam` |
| `executor::Executor` | `wasip2::Executor` |
| `executor::Mode` | `wasip2::Mode` |
| `executor::WaitPoll` | `wasip2::WaitPoll` |
| `executor::sleep` | `wasip2::sleep` |
| `request::RequestError` (Preview 2) | `wasip2::request::RequestError` |
| `request::RequestError` (Preview 3) | `::wasip3::http::types::ErrorCode` from `http_from_wasi_request` |
| `request::method_wasi_to_http` | `wasip2::request::method_wasi_to_http` |
| `request::scheme_wasi_to_http` | `wasip2::request::scheme_wasi_to_http` |
| `executor::Wasip3Executor` | `wasip3::Wasip3Executor` |
| `executor::init_wasip3_spawner` | `wasip3::init_wasip3_spawner` |
| `prelude::init_wasip3_spawner` | `wasip3::prelude::init_wasip3_spawner` |

The root `executor` and `request` modules are no longer public; supported
runtime exports live under `wasip2` or `wasip3`. The old Preview 2
`request::Request` wrapper and its `TryFrom` conversion were removed; call
`wasip2::request::from_wasi_request` with an explicit byte limit when direct
conversion is required. The old Preview 3 `request::Request` wrapper was unused
and has also been removed; convert the host request with
`wasip3::http_compat::http_from_wasi_request` as shown in the README.
`handler::WasiBuf` is now transport-private. The body-projection helper
`handler::ServerWithBody` moved to the explicitly unstable, doc-hidden
`__private` namespace and must not be named by application code.

`WaitPoll` and `sleep` now return `Result` so cancellation and missing-executor
conditions can be handled instead of trapping. Propagate those failures with
`?` or match `ExecutorError` explicitly.

## 2. Use the canonical server-function registration method

Replace both removed aliases with `with_server_fn`:

```rust
// 0.3
handler.with_server_fn_axum::<SaveTodo>();
handler.with_server_fn_generic::<SaveTodo>();

// 0.4
handler.with_server_fn::<SaveTodo>();
```

Request and response body types remain inferred. Axum-compatible generated body
types are an internal Leptos compatibility detail; no Axum HTTP server is
started. Version 0.4 does not expose or test a generic-response backend. A true
Axum-free `WasiServerFnBackend` requires a dedicated request newtype and macro
support because upstream's generic `Request<Bytes>` fixes its WebSocket response
to `Response<Bytes>`.

## 3. Propagate registration errors

Static-prefix and Leptos route registration now return
`Result<Self, RegistrationError>` instead of panicking:

```rust
let handler = Handler::build(request, response_out)?
    .static_files_handler("/pkg", serve_static_files)?
    .with_server_fn::<SaveTodo>()
    .generate_routes(App)?;
```

The fallible route methods are:

- `generate_routes`
- `generate_routes_with_discovery_context`
- `generate_routes_with_exclusions_and_discovery_context`

Registration rejects invalid static prefixes, repeated or colliding generated
routes, invalid route patterns, and unsupported `SsrMode::Static` routes.
Because `RegistrationError` is non-exhaustive, match it with a wildcard arm.

The static-file callback now always receives a normalized relative path. Remove
workarounds that strip a leading slash. Keep filesystem symlink containment in
the callback or serve production assets through the host/CDN.

## 4. Update executor initialization

The Preview 2 spelling error was corrected:

```rust
// 0.3
Mode::Premptive

// 0.4
Mode::Preemptive
```

`WasiExecutor::new` and `WasiExecutor::run_until` now return `Result`. Prefer
the runtime initializer, which installs and caches the thread-local executor:

```rust
use leptos_wasi::wasip2::prelude::{Mode, init_wasip2_executor};

let executor = init_wasip2_executor(Mode::Stalled)?;
let result = executor.run_until(request_future)?;
```

Calls using the same mode reuse the installed executor. A conflicting mode or
another installed global spawner returns a persistent error. Propagate these
errors and handle `ExecutorError::Stalled` rather than trapping silently.

Preview 3 initialization also returns a persistent result:

```rust
use leptos_wasi::wasip3::prelude::init_wasip3_spawner;

init_wasip3_spawner()?;
```

Do not discard this result. Repeated calls after successful initialization are
safe; if the first call conflicts with another installed global executor, every
later call returns the same conflict.

## 5. Configure request limits when needed

Both runtime handlers retain the 16 MiB default. Use an explicit policy for a
smaller application limit:

```rust
use leptos_wasi::prelude::HandlerConfig;

let config = HandlerConfig::default()
    .with_max_request_body_size(2 * 1024 * 1024);
```

Preview 2:

```rust
let handler = Handler::build_with_config(request, response_out, config)?;
```

Preview 3:

```rust
let handler = Handler::build_with_config(request, config).await?;
```

Oversized bodies receive 413. Invalid or conflicting Content-Length values
receive 400.

## 6. Stop constructing response parts with struct literals

`ResponseParts` is non-exhaustive and its fields are private. Construct it with
`ResponseParts::default()`, inspect it through `headers()` and `status()`, and
modify it through `headers_mut`, `set_status`, `clear_status`, `insert_header`,
or `append_header`. Install the completed value with
`ResponseOptions::overwrite`; direct `ResponseOptions` mutation remains
available through `set_status`, `insert_header`, and `append_header`.

## 7. Remove the obsolete example and unsupported assumptions

`examples/spin-counter` was removed. Use `examples/counter`, which is a
session-scoped demonstration of SSR, lazy islands, split browser WASM, server
functions, Wasmtime, and Spin.

The following are not part of the 0.4 contract:

- WebSockets
- streaming incoming request bodies
- HTTP response trailers
- `SsrMode::Static` generation
- byte-range responses
- automatic precompressed asset negotiation

See [PRODUCTION.md](./PRODUCTION.md) before deploying public traffic.

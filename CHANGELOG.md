# Changelog

All notable changes to this project will be documented in this file.

## [0.4.0] — 2026-07-10

### Added

- Additive `leptos_wasi::wasip2` and `leptos_wasi::wasip3` runtime namespaces,
  each with a runtime-specific prelude and handler.
- `HandlerConfig` and `build_with_config` with a configurable buffered request
  limit. The default remains 16 MiB.
- Non-exhaustive `RegistrationError` values for invalid static prefixes,
  repeated or colliding generated routes, invalid route patterns, and
  unsupported static SSR.
- Optional `tracing` instrumentation for request lifecycle spans and completion
  events without installing a subscriber.
- Preview 2 pollable registration IDs, cancellation cleanup, stalled-executor
  detection, and native cancellation unit tests.
- `init_wasip2_executor`, which installs and reuses one thread-local executor
  while persistently reporting task-spawner or mode conflicts.
- A migration guide, production support contract, explicit MSRV/current-stable
  CI, and independent WASIp2, WASIp3, and dual-feature verification lanes.
- A recorded 0.3.2 performance baseline and ten-minute CI soak matrix for
  Wasmtime/Spin and Preview 2/Preview 3.

### Changed

- The root prelude now exports only shared response, configuration, status, and
  redirect types. Import `Handler` and executor types from a runtime namespace.
- `static_files_handler` and all route-generation builders now return
  `Result<Self, RegistrationError>` instead of panicking on invalid input.
- Preview 2 `Executor::new` and `run_until` are fallible, and
  `init_wasip3_spawner` persistently returns the first initialization outcome.
- `Mode::Premptive` is corrected to `Mode::Preemptive`.
- `ResponseParts` is non-exhaustive with private fields and supported accessor
  and `ResponseOptions` mutation methods.
- The counter example is session-scoped and focuses on SSR, lazy islands,
  split browser WASM, typed server functions, Wasmtime, and Spin.
- WASIp2 and WASIp3 features are additive. Enabling both exposes both adapters
  rather than silently selecting Preview 3.

### Fixed

- Server-function middleware now executes in Leptos layer order, including
  authentication rejection, response mutation, and middleware error encoding.
- Static asset paths are decoded once and confined to normalized relative
  paths. Encoded separators, absolute paths, dot segments, NUL, malformed
  escapes, and double-encoded control sequences are rejected.
- Static routes now implement GET/HEAD semantics, return 405 for other methods,
  and add `X-Content-Type-Options: nosniff`.
- SSR `RequestUrl` preserves the query string and standard Leptos contexts are
  installed before application-provided context.
- Request and host failures that can be handled before response commitment no
  longer trap through request-reachable `expect`/`panic` paths.
- Preview 3 response frames no longer require `Vec::drain`, and Preview 2 no
  longer flushes after every 8 KiB write.

### Removed

- `with_server_fn_axum` and `with_server_fn_generic`; use the canonical
  `with_server_fn::<T>()` method.
- The ambiguous root `prelude::Handler`, public `WasiBuf`, and unused Preview 3
  request wrapper.
- The obsolete `examples/spin-counter` application.

### Known limitations

- Incoming request bodies remain buffered. WebSockets, request-body streaming,
  HTTP trailers, static SSR generation, byte ranges, and automatic
  precompressed asset negotiation are not supported. A native Axum-free
  server-function backend also remains post-0.4 experimental. See
  `PRODUCTION.md`.

## [0.3.2] — 2026-07-10

### Added

- A dual-runtime `examples/counter` implementation of Leptos islands with a
  lazy `#[island(lazy)]` browser chunk and `cargo leptos --split` build flow.
- Split-asset verification and runtime mounts that make the browser package and
  split manifest available under both Wasmtime and Spin.

### Fixed

- The `islands-router` integration now mirrors Leptos's request contract under
  WASIp2 and WASIp3: it detects the `Islands-Router` header, provides
  `IslandsRouterNavigation`, and disables out-of-order rendering for navigation
  responses.

---

## [0.3.1] — 2026-05-29

### Changed

- **Restored single type parameter API**: `with_server_fn::<T>()` now requires only the server function type `T`, eliminating the need for `.with_server_fn::<T, _>()` or explicit body type parameters introduced in 0.2.0. This is achieved via the `ServerWithBody` helper trait which projects body types from the server implementation at compile time with zero runtime overhead.
- Updated `examples/counter` and `examples/spin-counter` to use the simplified `.with_server_fn::<T>()` API.

### Added

- **Security Hardening**:
  - 16MB request payload limit (`MAX_REQUEST_BODY_SIZE`) to prevent heap exhaustion inside the WebAssembly guest.
  - `sanitize_referrer` — blocks protocol-relative redirects via backslashes (`\`) and URL-encoded backslashes (`%5c`, `%5C`).
  - Route hijacking prevention via segment-boundary matching in the static file handler.
  - Graceful `400 Bad Request` on invalid headers instead of guest panics.
  - Percent-encoded static file name support.
- **E2E Test Suite**: Host-driven integration tests (`tests/e2e.rs`) and a guest test app (`tests/test-app/`) validating server functions, SSR modes, static files, redirects, payload limits, and panic containment under both `wasip2` and `wasip3` runtimes via `wasmtime serve`.
- `run_tests.sh` — compiles guest targets and runs the full E2E suite.

---

## [0.3.0] — 2026-05-28

### Added

- **Native WASI Preview 3 support** via the `wasip3` feature flag. Task spawning is delegated directly to the host runtime using `wasip3::wit_bindgen::spawn`, eliminating guest-side cooperative polling overhead.
- `init_wasip3_spawner()` public API for initializing the WASIp3 executor at the server entrypoint.
- Async `Handler::build()` signature under WASIp3 (takes `http::Request` instead of WASI-specific request/response types).
- Spin SDK v6 / Spin v4 compatibility with native WASIp3 HTTP triggers.
- `http_body::Body` implementation for `crate::response::Body` under WASIp3.
- `examples/counter` — dual-runtime example supporting both raw Wasmtime and Spin via compile-time `build.rs` runtime detection.
- `examples/spin-counter` — Spin-only example using Spin's built-in key-value store.
- `rust-version = "1.85.0"` (MSRV) — required for edition 2024.

### Changed

- **Upgraded Leptos ecosystem:**
  - `leptos` → `0.8.9`
  - `server_fn` → `0.8.7`
  - `leptos_router` → `0.8.7`
  - `leptos_meta` → `0.8.5`
  - `leptos_macro` → `0.8.8`
- Stripped semver build metadata from `wasi` (`0.13.1+wasi-0.2.0` → `0.13.1`) and `wasip3` (`0.6.0+wasi-0.3.0-rc-2026-03-15` → `0.6.0`) version requirements to eliminate Cargo warnings.
- `std::io::Error::new(ErrorKind::Other, ...)` → `std::io::Error::other(...)` in `handler.rs` and `response.rs` (clippy `io_other_error`).
- Deprecated `try_next()` → `try_recv()` in `executor.rs` (clippy `while_let_loop`).
- Formatting fixes in `request.rs`.

### Removed

- Legacy WASIp2 examples (`examples/counter` and `examples/spin-counter` — the old cooperative polling versions).
- Vendored `./bin/` directory and all references to local binary tooling.
- Hardcoded relative binary paths (`../../bin/spin`, `../../bin/wasmtime`) from Makefiles.

### Dependencies Added

- `wasip3 = "0.6.0"` — WASIp3 core types, host spawner bindings, HTTP compatibility layers.
- `http-body = "1.0.0"` — standard streaming response frames for the WASIp3 pipeline.
- `http-body-util = "0.1.3"` — body utilities.
- `axum-core = "0.5.2"` — axum backend support for server functions.

## [0.2.0]

### Changed

- Server function registration API: `with_server_fn::<T>()` (single type param) → `with_server_fn::<T, B>()` (explicit body type) to support generic request/response body types for streaming.
- Added convenience methods: `with_server_fn_axum::<T>()` and `with_server_fn_generic::<T>()`.
- Static file handler signature updated to return `Option<leptos_wasi::response::Body>` directly.

## [0.1.3]

### Added

- Initial server function registration with single type parameter API: `.with_server_fn::<T>()`.
- WASIp2 cooperative polling executor.
- Basic static file serving and SSR support.

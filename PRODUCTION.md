# Production Support

This document defines the supported `leptos_wasi` 0.4 contract and the release
gate for calling a build production-ready. It does not replace the Wasmtime or
Spin host security model.

## Support matrix

| Capability | Wasmtime WASIp2 | Spin WASIp2 | Wasmtime WASIp3 | Spin WASIp3 |
|---|---:|---:|---:|---:|
| Leptos SSR routes | Yes | Yes | Yes | Yes |
| Typed server functions | Yes | Yes | Yes | Yes |
| Server-function middleware | Yes | Yes | Yes | Yes |
| Standard component middleware | No | No | Experimental | Experimental |
| Streaming response bodies | Yes | Yes | Yes | Yes |
| GET/HEAD static callback | Yes | Yes | Yes | Yes |
| Islands and split browser WASM | Server compatible | Server compatible | Browser E2E | Browser E2E |
| Incoming request streaming | No | No | No | No |
| WebSockets | No | No | No | No |
| HTTP response trailers | No | No | No | No |
| `SsrMode::Static` generation | No | No | No | No |
| Byte ranges/precompressed negotiation | No | No | No | No |

Both runtime features may be enabled in one dependency graph. The application
still exports a host entrypoint for the component model it intends to run.
Axum-compatible generated server-function body types remain an internal
dependency in 0.4. A native Axum-free WASI backend is experimental and outside
this support matrix.

## Request and response contract

- Incoming bodies are buffered with a configurable maximum. The default is
  16 MiB. Content-Length is checked before body collection when present, and
  the collected size is checked independently.
- Invalid or conflicting Content-Length values receive 400. Bodies over the
  configured limit receive 413.
- Request deadlines are a deployment responsibility. Configure them at the
  Wasmtime/Spin ingress because a guest cannot reliably cancel a client that
  the host continues to feed.
- Server-function middleware executes in Leptos order. Authentication,
  authorization, rate limiting, and tracing layers must still be supplied by
  the application, a composed component, or the ingress at the appropriate
  scope.
- WASIp3 component middleware is currently an experimental compatibility path,
  not part of the stable 0.4 support claim. Middleware must strip untrusted
  identity headers before adding validated identity metadata, and only the
  outer composed handler may be externally routable.
- Generated route discovery is cached by the concrete application/context
  closure types. Keep route structure and exclusion lists deterministic
  deployment configuration; do not derive them from request data.
- Host failures before response commitment are converted to controlled HTTP
  failures. A stream failure after commitment terminates that response because
  its status can no longer be changed.
- Internal host errors are not returned verbatim to clients. Capture details
  through host logs or the optional `tracing` feature.

## Static assets

The static callback receives a percent-decoded, normalized relative path.
Absolute paths, root/prefix components, dot segments, NUL, encoded separators,
double-encoded control sequences, and traversal are rejected. Static routes
accept GET and HEAD; other methods receive 405.

Path normalization cannot prove filesystem containment across symlinks. Use
one of these deployment patterns:

1. Serve assets from the host or a CDN and do not register a guest static
   callback.
2. Mount a dedicated read-only directory without attacker-controlled symlinks.
3. Canonicalize the asset root and candidate in the callback and verify that
   the candidate remains below the root before reading it.

Do not grant the component broader filesystem preopens than its callback needs.
Each host HTTP trigger has its own middleware stack. A middleware dependency on
the Leptos trigger does not cover a separate static-file trigger or CDN. Keep
browser loader, main WASM, and `split_*.wasm` assets public unless the asset
service has an explicit authentication bypass.

## Runtime operations

Configure these controls outside the crate:

- request and response deadlines;
- maximum concurrent component invocations;
- component memory and table limits;
- read-only filesystem preopens;
- ingress body limits no larger than the guest limit;
- TLS, forwarded-header trust, and client IP policy;
- log retention, metrics collection, and alerts.

WASIp2 uses a single-threaded cooperative executor. Initialize it through
`init_wasip2_executor`, which caches the thread-local executor and rejects a
mode mismatch. Treat `ExecutorError::Stalled` as an operational failure and
monitor canceled/live pollable counts when tracing is enabled.

WASIp3 delegates tasks to the host. Call `init_wasip3_spawner()` before serving
the first request and propagate a persistent initialization conflict. Do not
discard its result.

The experimental WASIp3 component-middleware tuple is pinned in
`tests/middleware/components.lock.toml`. Do not deploy a floating Spin or SDK
branch. Promote component middleware into this support matrix only after a
stable Spin release and the application bindings use the same WIT revision.
The independently versioned `wasi-http-middleware 0.1.0-alpha.1` companion owns
the reusable request-ID, security-header, CORS, and external-auth policy chain;
the local fixture is intentionally limited to ABI, request-context, streaming,
browser-auth, and split-WASM compatibility.

## Observability and sensitive data

The optional `tracing` feature emits a `leptos_wasi.request` span with the WASI
runtime family, preview, method, path without its query, route class, SSR mode,
accepted request ID, and request bytes. Its transport-completion event records
status, response bytes, total duration, first-byte time, cancellation, and a
bounded error class. The crate does not install a subscriber; the
application/host chooses a subscriber and export format.

Do not record bodies, authorization headers, cookies, or raw query strings.
When accepting an incoming request ID, validate its character set and length;
otherwise generate or assign correlation at the ingress. Alerts should cover:

- unexpected 5xx and post-commit stream failures;
- body-limit and malformed-request rejection rates;
- request duration and first-byte latency;
- WASIp2 stalls, queue depth, and cancellations;
- component traps, restarts, and memory-limit violations.

## Dependency policy

Every release runs `cargo audit` and rejects known vulnerabilities. The 0.4
dependency graph currently contains two transitive crates with unmaintained
advisories, not known vulnerability advisories:

| Advisory | Transitive crate | Introduced through | Owner | Compensating control | Removal condition |
|---|---|---|---|---|---|
| `RUSTSEC-2024-0436` | `paste` | Leptos/Tachys | `leptos_wasi` maintainers | This is an unmaintained warning, not a known vulnerability; the lockfile is reviewed and CI denies every warning outside this exact allowlist. | Remove the ignore when Leptos/Tachys no longer resolves `paste`. |
| `RUSTSEC-2026-0173` | `proc-macro-error2` | Leptos macro stack | `leptos_wasi` maintainers | This is an unmaintained warning, not a known vulnerability; the lockfile is reviewed and CI denies every warning outside this exact allowlist. | Remove the ignore when the Leptos macro stack no longer resolves `proc-macro-error2`. |

These exceptions must be reviewed whenever the Leptos dependency set changes
and before every release. Do not add a vulnerability ignore without documenting
the affected path, compensating controls, owner, and removal condition here.

## Load and soak probe

From a repository checkout, start the target Spin or Wasmtime deployment and
run the release probe:

```bash
python3 scripts/load_runtime.py http://127.0.0.1:3000/ \
  --duration 600 \
  --concurrency 100 \
  --pid <host-pid>
```

The probe reports request rate, status counts, failures, first-byte and
completed-response p50/p95/p99/max latency, and optional host RSS samples. It
exits unsuccessfully when any request fails or returns a non-2xx/3xx response.
Run it against each host/preview combination and retain the JSON with the
release evidence. Omit `--pid` when host RSS is collected by deployment
monitoring. The final-quarter RSS growth is a diagnostic; a release reviewer
must still inspect the time series or host telemetry before declaring a stable
memory plateau.

## Release gate

A production release requires all of the following:

- Formatting, Clippy, tests, and rustdoc pass for WASIp2, WASIp3, and both
  features together.
- Rust 1.93.0 and current stable pass the feature matrix.
- Wasmtime and Spin E2E pass for both previews.
- Raw encoded-path security cases, middleware, SSR query context, body limits,
  cancellation, disconnect, delayed stream, and mid-stream failure are tested.
- Preview 3 browser E2E proves SSR output, an initially unhydrated island, lazy
  split-WASM fetch, hydration, and interaction on Wasmtime and Spin.
- A ten-minute steady-load soak reaches a memory plateau, leaves no orphaned
  WASIp2 pollables, and produces no unexpected 5xx.
- Release-mode p99 latency regresses no more than 5% from the recorded baseline.
- `cargo package --locked`, `cargo publish --dry-run --locked`, and
  `cargo audit` pass.
- Every breaking API change appears in [MIGRATION.md](./MIGRATION.md).

The non-blocking vNext middleware lane is additional compatibility evidence; it
does not replace any stable-runtime gate above. See [WASIp3 HTTP
Middleware](./MIDDLEWARE.md).

Run the local compile/test/documentation matrix with:

```bash
cargo make ci
```

The checked-in local comparison and reproduction details are in
[PERFORMANCE.md](./PERFORMANCE.md). CI soak artifacts are the authoritative
evidence for a particular commit and release environment.

# Production Support

This document defines the supported `leptos_wasi` 0.5 alpha contract and the
release gate for calling a future stable build production-ready. It does not
replace the Wasmtime or Spin host security model.

## Support matrix

Runtime labels are separate from crate stability: **production** means the
complete correctness, browser, performance, and soak gates passed;
**reference** means correctness-compatible without a production latency claim;
**experimental** is interoperability-only; and **blocked upstream** means the
tagged runtime cannot link the final application ABI.

| Capability | Wasmtime WASIp2 | Spin WASIp2 | Wasmtime WASIp3 | Spin WASIp3 |
|---|---:|---:|---:|---:|
| Leptos SSR routes | Yes | Yes | Yes | Blocked by host linker |
| Typed server functions | Yes | Yes | Yes | Blocked by host linker |
| Server-function middleware | Yes | Yes | Yes | Blocked by host linker |
| Standard component middleware | No | No | Experimental alpha; performance-blocked | Canary only |
| Streaming response bodies | Yes | Yes | Yes | Blocked by host linker |
| GET/HEAD static callback | Yes | Yes | Yes | Blocked by host linker |
| Islands and split browser WASM | Server compatible | Server compatible | Browser E2E | Canary only |
| Incoming request streaming | No | No | No | No |
| WebSockets | No | No | No | No |
| HTTP response trailers | No | No | No | No |
| `SsrMode::Static` generation | No | No | No | No |
| Byte ranges/precompressed negotiation | No | No | No | No |

Both runtime features may be enabled in one dependency graph. The application
still exports a host entrypoint for the component model it intends to run.
Axum-compatible generated server-function body types remain an internal
dependency in the 0.5 alpha. A native Axum-free WASI backend is experimental and outside
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
  not part of the stable support claim. Middleware must strip untrusted
  identity headers before adding validated identity metadata, and only the
  outer composed handler may be externally routable.
- Generated route discovery is cached by the concrete application/discovery
  context closure types. Keep route structure, discovery context, and exclusion
  lists deterministic deployment configuration. Request-dependent context must
  be installed through `handle_with_context`, never a route-generation method.
- Identity observed while rendering SSR is presentation state only. Hiding a
  control or route link is never authorization. Protected server functions must
  re-read trusted per-request context and enforce typed policy after
  deserialization; synthetic route-discovery context must never participate in
  authentication or authorization.
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

The stock `wasmtime serve` CLI is the final-WASI correctness reference. Its
current Preview 3 outbound client opens fresh connections and has not passed
the concurrency-100 authorization latency gate, so it carries no production
performance claim. It needs inherited networking for the precomposed
authentication client and does not provide a per-host HTTP allowlist. Enforce
the exact broker destination in a custom Wasmtime embedding or an outbound
network sandbox before production. The local loopback runner validates the
protocol and fail-closed behavior, not network egress isolation.

WASIp2 uses a single-threaded cooperative executor. Initialize it through
`init_wasip2_executor`, which caches the thread-local executor and rejects a
mode mismatch. Treat `ExecutorError::Stalled` as an operational failure and
monitor canceled/live pollable counts when tracing is enabled.

WASIp3 delegates tasks to the host. Call `init_wasip3_spawner()` before serving
the first request and propagate a persistent initialization conflict. Do not
discard its result.

The final `wasi:http@0.3.0` component tuple is pinned in
`tests/middleware/components.lock.toml`. Wasmtime 46.0.1 runs the deterministic
WAC-precomposed chain. Stable Spin 4 cannot link the final
`wasi:http/types@0.3.0` resources, while Spin's native middleware commit still
imports the March RC handler world. The precomposed runtime lane and native
`dependencies.middleware` lane therefore remain expected-incompatibility
canaries. Promote Spin only after a tagged release provides final handler,
types, and client host support plus native middleware composition against the
final WIT. Do not downgrade production components to the RC or deploy floating
tool/runtime revisions. The independently versioned middleware companion owns
the request-ID, security-header, CORS, authentication, spoof stripping, and
credential-removal components; `wasi-authz` owns typed application policy.

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

Every release runs `cargo audit` and `cargo deny check`. The checked
[`deny.toml`](./deny.toml) restricts registry sources, rejects yanked crates and
wildcard requirements, makes duplicate versions visible, and permits only the
reviewed license set. The 0.5 alpha dependency graph currently contains two
transitive crates with unmaintained advisories, not known vulnerability
advisories:

| Advisory | Transitive crate | Introduced through | Owner | Compensating control | Removal condition |
|---|---|---|---|---|---|
| `RUSTSEC-2024-0436` | `paste` | Leptos/Tachys | `leptos_wasi` maintainers | This is an unmaintained warning, not a known vulnerability; the lockfile is reviewed and CI denies every warning outside this exact allowlist. | Remove the ignore when Leptos/Tachys no longer resolves `paste`. |
| `RUSTSEC-2026-0173` | `proc-macro-error2` | Leptos macro stack | `leptos_wasi` maintainers | This is an unmaintained warning, not a known vulnerability; the lockfile is reviewed and CI denies every warning outside this exact allowlist. | Remove the ignore when the Leptos macro stack no longer resolves `proc-macro-error2`. |

These exceptions must be reviewed whenever the Leptos dependency set changes
and before every release. Do not add a vulnerability ignore without documenting
the affected path, compensating controls, owner, and removal condition here.

## Load and soak probe

From a repository checkout, run the trusted-ingress release probe:

```bash
DURATION=600 CONCURRENCY=100 ./scripts/soak-trusted-ingress.sh
```

The Rust `trusted-load` driver keeps its Hyper pool across warmup and
measurement, consumes bodies and trailers, separates response-header,
first-body-byte, and total latency, and samples every topology process.
`scripts/load_runtime.py` remains a legacy transport probe and is not valid
promotion evidence.

## Release gate

A stable library release requires the following runtime-independent gates;
deployment promotion is tracked separately:

- Formatting, Clippy, tests, and rustdoc pass for WASIp2, WASIp3, and both
  features together.
- Rust 1.93.0 and current stable pass the feature matrix.
- Wasmtime E2E passes for both previews; Spin E2E passes for Preview 2, and the
  Preview 3 expected-failure canary still matches the pinned linker failure.
- Raw encoded-path security cases, middleware, SSR query context, body limits,
  cancellation, disconnect, delayed stream, and mid-stream failure are tested.
- Preview 3 browser E2E proves SSR output, an initially unhydrated island, lazy
  split-WASM fetch, hydration, and interaction on Wasmtime.
- The checksum-pinned sibling middleware checkout is clean at the exact source
  revision; its checksum manifest and provenance contain every declared
  production component exactly once; pinned Cosign verification succeeds for
  the bound provenance and OCI manifest; its full Wasmtime chain and browser
  runner pass locally; and a remote or release-artifact source exists before CI
  promotion. Until remote or artifact distribution is authorized, this
  cross-repository gate is a release blocker rather than a skipped CI success.
- Delayed first byte, trailers, disconnect, and a committed frame followed by
  an intentional terminal stream error pass through the composed chain without
  buffering or rewriting the committed status.
- A ten-minute steady-load soak reaches a memory plateau, leaves no orphaned
  WASIp2 pollables, and produces no unexpected 5xx.
- Release-mode p99 latency regresses no more than 5% from the recorded baseline.
- `cargo package --locked`, `cargo publish --dry-run --locked`, and
  `cargo audit` pass.
- Every breaking API change appears in [MIGRATION.md](./MIGRATION.md).

Wasmtime remains a blocking correctness/reference gate, not a 25 ms production
latency gate. Spin becomes the production-performance target only after a
tagged release links final `wasi:http@0.3.0`; it must then pass five paired
5,000-request concurrency-100 repetitions and the ten-minute mixed soak. Until
then the Spin Preview 3 checks are upstream compatibility canaries. See
[WASIp3 HTTP Middleware](./MIDDLEWARE.md).

Run the local compile/test/documentation matrix with:

```bash
cargo make ci
```

The checked-in local comparison and reproduction details are in
[PERFORMANCE.md](./PERFORMANCE.md). CI soak artifacts are the authoritative
evidence for a particular commit and release environment.

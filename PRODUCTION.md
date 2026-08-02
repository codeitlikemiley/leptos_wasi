# Production Support

This document defines the supported `leptos_wasi` 0.4.2 alpha contract and the
release gate for calling a future stable build production-ready. It does not
replace the Wasmtime or Spin host security model.

## Support matrix

Runtime labels are separate from crate stability: **production** means the
complete correctness, browser, performance, and soak gates passed;
**reference** means correctness-compatible without a production latency claim;
**experimental** is interoperability-only; and **blocked upstream** means the
tagged runtime cannot link the final application ABI.

| Capability | Wasmtime WASIp2 | Spin WASIp2 | Wasmtime WASIp3 | Tagged Spin 4.0.2 WASIp3 | Pinned Spin main WASIp3 |
|---|---:|---:|---:|---:|---:|
| Leptos SSR routes | Yes | Yes | Yes | Blocked by host linker | Experimental pass |
| Typed server functions | Yes | Yes | Yes | Blocked by host linker | Experimental pass |
| Server-function middleware | Yes | Yes | Yes | Blocked by host linker | Experimental pass |
| Standard component middleware | No | No | Experimental alpha; performance-blocked | Blocked by host linker | CPU-metrics panic / native RC-only |
| Streaming response bodies | Yes | Yes | Yes | Blocked by host linker | Experimental pass |
| GET/HEAD static callback | Yes | Yes | Yes | Blocked by host linker | Experimental pass |
| Islands and split browser WASM | Server compatible | Server compatible | Browser E2E | Blocked by host linker | Browser E2E |
| SQLite counter client | N/A | N/A | Cross-runtime E2E | Blocked by host linker | Cross-runtime E2E |
| Incoming request streaming | No | No | No | No | No |
| WebSockets | No | No | No | No | No |
| HTTP response trailers | No | No | No | No | No |
| `SsrMode::Static` generation | No | No | No | No | No |
| Byte ranges/precompressed negotiation | No | No | No | No | No |

Both runtime features may be enabled in one dependency graph. The application
still exports a host entrypoint for the component model it intends to run.
Axum-compatible generated server-function body types remain an internal
dependency in the 0.4.2 alpha. A native Axum-free WASI backend is experimental and outside
this support matrix.

## Request and response contract

- Incoming bodies are buffered with a configurable maximum. The default is
  16 MiB. Content-Length is checked before body collection when present, and
  the collected size is checked independently.
- Invalid or conflicting Content-Length values receive 400. Bodies over the
  configured limit receive 413.
- Body size is bounded on both previews; body read time is bounded on neither.
  Preview 2 collects the body with a blocking-read loop over the WASI input
  stream and Preview 3 awaits a length-limited `collect`. Both stop at the
  configured byte limit, and both end when the client closes the connection.
  Neither imposes a deadline of its own, so a client that holds the connection
  open while sending slowly, or stops sending without closing, occupies the
  guest instance until the host terminates the request.
- Request deadlines are therefore a deployment responsibility. Configure a
  request-read/idle deadline and a cap on concurrent component invocations at
  the Wasmtime/Spin ingress, not only a body-size limit; a size limit does not
  bound how long one request holds an instance, and a guest cannot reliably
  cancel a client that the host continues to feed.
- `HandlerConfig::with_request_body_timeout_ns` adds an optional guest-side
  budget for the whole body, off by default. It is defense in depth for a
  deployment whose ingress cannot supply a read deadline, not a replacement
  for one: the guest still cannot cancel a client the host keeps feeding, and
  enabling it converts a slow upload into `408 Request Timeout`. The budget
  spans the whole body rather than the gap between chunks, so both previews
  mean the same thing by it and a client trickling one byte at a time cannot
  refresh it.
- Route discovery does not run on requests that cannot use the SSR router. A
  server function, a static asset, and an already-selected response all resolve
  without it, and discovery renders the whole application, so skipping it is
  worth about 183 us on those requests. The consequence is that a malformed
  route table is no longer rejected while serving such a request: an
  application reached only through server functions and static assets never
  validates its routes in production. Call `leptos_wasi::validate_route_table`
  once from a test to close that gap. It applies exactly the rules the request
  path applies and shares their implementation, so the two cannot drift.
- Server-function middleware executes in Leptos order. Authentication,
  authorization, rate limiting, and tracing layers must still be supplied by
  the application, a composed component, or the ingress at the appropriate
  scope.
- A redirect carried by a server-function response is reduced to a same-origin
  path before the response leaves the handler. The request `Referer`
  (or `Referrer`) header and any `Location` already on the server-function
  response go through the same check: the value must parse as a URI, must not
  contain a backslash or an encoded backslash (`%5c`/`%5C`), and its
  path-and-query must begin with a single `/`. Anything else — an absolute
  `https://` URL, a protocol-relative `//host/path` — is replaced with `/`.
- That reduction covers the server-function path only. A `Location` written
  from reactive context through `ResponseOptions`, which is what
  `leptos_wasi::prelude::redirect` does, is merged into the response after that
  step and is sent as written, including an absolute off-origin URL.
  Applications that build a redirect target from request data should validate
  it before passing it to `redirect` or `ResponseOptions::insert_header`, or
  constrain the allowed origins at the ingress.
- Both behaviours differ from `leptos_axum` and `leptos_actix`, which pass
  server-function `Location` values through unchanged. An application ported
  from those integrations may find that a cross-origin server-function redirect
  which worked there is rewritten to `/` here, while the `ResponseOptions` path
  behaves as it did upstream.
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
- A per-request Leptos nonce is provided with the standard contexts and is
  applied to the inline hydration and streaming scripts the handler emits. No
  `Content-Security-Policy` header is sent by the crate. An application that
  wants a nonce-based policy writes the header itself through
  `ResponseOptions` during the first rendered chunk; see [Content Security
  Policy](./README.md#content-security-policy). Header-only policies with no
  per-request input belong at the ingress with the other response security
  headers.

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

Guest-served static responses set `Content-Type`, `X-Content-Type-Options:
nosniff`, and, for a synchronous body, `Content-Length`. They set no
`Cache-Control`, `ETag`, or `Last-Modified`, and the handler does not evaluate
conditional requests, so `If-None-Match` and `If-Modified-Since` are ignored
and no `304 Not Modified` is produced. Every request re-reads and re-sends the
full body, and with no explicit freshness information a browser or intermediary
may apply heuristic caching — which is the opposite of what the unhashed `/pkg`
assets this flow deploys need. The two supported deployments already differ
here: the Spin manifests route `/pkg/...` to `spin-fileserver` with
`CACHE_CONTROL = "no-cache"`, while the Wasmtime guest callback path sends
nothing. Serving assets from a host fileserver or CDN, as in pattern 1 above,
also supplies the caching and revalidation headers the guest callback does not.

Do not grant the component broader filesystem preopens than its callback needs.
Each host HTTP trigger has its own middleware stack. A middleware dependency on
the Leptos trigger does not cover a separate static-file trigger or CDN. Keep
browser loader, main WASM, and `split_*.wasm` assets public unless the asset
service has an explicit authentication bypass.

## Runtime operations

Configure these controls outside the crate:

- request-read, idle, and response deadlines, since the guest bounds body size
  but not body read time;
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

### Component instance reuse

`wasmtime serve` bounds how many requests one component instance serves with
`--max-instance-reuse-count`, and it **defaults to 1 for WASIp2 and 128 for
WASIp3**. A Preview 2 deployment therefore instantiates a fresh component for
every request unless it opts out, which measures as roughly 107 us of avoidable
work per request — about **14% of throughput** at low concurrency. That is
larger than any single change this crate has made to its own request path.

Raising it is the highest-leverage tuning available to a Preview 2 deployment:

```
wasmtime serve --max-instance-reuse-count 128 app.wasm
```

The pooling allocator (`-O pooling-allocator=y`) was measured alongside it and
is indistinguishable from noise, with or without reuse. It is not part of this
recommendation.

Reuse is not free of consequences, and the consequence is the reason to test
before adopting it. A reused instance keeps its guest statics between requests,
so anything an application stores in a `static` or `thread_local` now outlives
the request that created it. This crate's own statics — the Preview 2 executor
cell and the pollable queue — are designed to be reused and pass the end-to-end
suite under `--max-instance-reuse-count 128`, covering server functions, static
assets, SSR, islands, redirects, and a panicking server function. An
application that keeps request-scoped state in a static will leak it between
requests, and no test in this repository can detect that for you. Run your own
suite under reuse before enabling it; `LEPTOS_WASI_MAX_INSTANCE_REUSE` wires
the flag into this repository's e2e tests for that purpose.

Other hosts expose the same trade-off under different names. Reuse also changes
what a cold start means for observability: a latency histogram under reuse has
a small population of instantiating requests rather than a uniform floor.

WASIp2 uses a single-threaded cooperative executor. Initialize it through
`init_wasip2_executor`, which caches the thread-local executor and rejects a
mode mismatch. Treat `ExecutorError::Stalled` as an operational failure and
monitor canceled/live pollable counts when tracing is enabled.

WASIp3 delegates tasks to the host. Call `init_wasip3_spawner()` before serving
the first request and propagate a persistent initialization conflict. Do not
discard its result.

The final `wasi:http@0.3.0` component tuple is pinned in
`tests/middleware/components.lock.toml`. Wasmtime 46.0.1 runs the deterministic
WAC-precomposed chain. Tagged Spin 4.0.2 cannot link the final
`wasi:http/types@0.3.0` resources. Pinned Spin `4.1.0-pre0` at `c34c584...`
runs plain terminals, final outbound HTTP, trusted ingress, islands, and the
SQLite persistence example, but is not a released runtime. Its default CPU
accounting still panics around WAC-composed handlers, while native middleware
still imports the March RC handler world. Promote Spin only after a tagged
release contains the working final-WASI path and fixes those middleware
limitations. Do not downgrade production components to the RC or deploy
floating tool/runtime revisions. The independently versioned middleware companion owns
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
reviewed license set. The 0.4.2 alpha dependency graph currently contains two
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
- Wasmtime E2E passes for both previews; Spin E2E passes for Preview 2; the
  tagged Preview 3 expected-failure canary matches its pinned linker failure;
  and the exact pinned-main terminal lane passes final-WASI browser behavior.
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

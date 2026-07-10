# WASIp3 HTTP middleware

`leptos_wasi` is a terminal `wasi:http/service` component. Whole-service
middleware is composed around that terminal component by the deployment; it is
not registered through a `leptos_wasi::Handler` builder method.

This keeps reusable authentication, CORS, request-ID, and response-header
components independent of Leptos. The same middleware artifact can wrap other
WASIp3 HTTP services, while each HTTP trigger selects its own ordered stack.
The companion `wasi-http-middleware` repository implements that reusable chain
as independently compiled components. Local integration runners verify and
copy checksum-pinned artifacts from the sibling checkout; `leptos_wasi` does
not carry a second protocol-only implementation.

## Scope and ordering

There are three distinct policy boundaries:

1. **WASIp3 component middleware** applies to the complete selected HTTP
   service. Use it for framework-neutral request and response policy.
2. **Leptos server-function middleware** registered through
   `with_server_fn::<T>()` applies to one typed server function. Use it for
   authorization or validation that depends on that endpoint.
3. **Host or ingress policy** owns TLS, WAF rules, trusted client addresses,
   request deadlines, concurrency, memory limits, and distributed rate limits.

A recommended outermost-to-innermost stack is:

```text
request-id -> security-headers -> cors -> authn-policy -> leptos service
```

When a service needs coarse HTTP admission as well, append the independently
versioned `wasi-authz` PEP after trusted authentication:

```text
request-id -> security-headers -> cors -> authn-policy -> authz-http-pep -> leptos service
```

`authz-http-pep` evaluates only the stable `http.request` action against the
configured immutable service ID, method, and normalized query-free path. It is
not a substitute for domain authorization: an order ID, ownership relation, or
other body-derived resource must still be checked by typed server-function
code after deserialization and resource loading.

For the Leptos authorization fixture, the coarse PEP is intentionally an
opt-in profile. The default `wasmtime-authn-authz` profile keeps only the
framework-neutral middleware and performs Cedar/SpiceDB domain checks in the
protected server function. Set `AUTHZ_COARSE_PEP=1` when validating the
additional service-admission hop with the `wasmtime-authn-authz-coarse`
profile. This avoids making every protected operation pay for both coarse path
policy and typed domain policy by default.

Requests travel from left to right and responses travel from right to left.
This ordering lets CORS, request-ID, and security-header middleware decorate an
authentication rejection without invoking the application.

Middleware components are composed in-process. They are separate WebAssembly
components, not separately deployed HTTP proxies, so the normal design does not
add a network hop.

## Spin composition

The application and middleware artifacts use final `wasi:http@0.3.0` bindings.
Spin's native middleware implementation at the pinned commit still hard-codes
`wasi:http/handler@0.3.0-rc-2026-03-15`; current upstream cannot compose the
final components. The experimental manifest in
[`tests/spin-p3-middleware-vnext.toml`](tests/spin-p3-middleware-vnext.toml)
therefore remains an expected-incompatibility canary for this shape:

```toml
[[trigger.http]]
route = "/..."
component = "leptos-app"
dependencies.middleware = [
  { component = "request-id" },
  { component = "security-headers" },
  { component = "cors", inherit_configuration = ["environment"] },
  { component = "authn-policy", inherit_configuration = ["environment", "allowed_outbound_hosts"] },
]
```

The dependency list is ordered outermost to innermost. It is not a production
path until a tagged Spin release supports final WASI HTTP. A second canary uses
the deterministic WAC-precomposed component through
[`tests/spin-p3-middleware-composed.toml`](tests/spin-p3-middleware-composed.toml).
Stable Spin 4 also rejects that component because its host linker does not
provide the final `wasi:http/types@0.3.0` resource implementation. Wasmtime 46
is therefore the only blocking final-WASI behavioral runtime in this release.
Production deployments must consume versioned artifacts pinned by digest. The
local runner accepts a sibling checkout only after checking the declared
compatibility tuple, artifact checksums, component WIT, SBOMs, provenance
subjects, OCI manifest, public key, and detached signature bundles. The sync
gate runs pinned Cosign verification for both the provenance statement and OCI
manifest before copying a component. The checked
`tests/middleware/artifact-sets.toml` record binds those exact local-alpha
files; its ephemeral development key is evidence for this local build, not a
production release identity.

The exact experimental runtime, SDK, WIT, and composition-tool revisions are
recorded in
[`tests/middleware/components.lock.toml`](tests/middleware/components.lock.toml).
Do not replace those revisions with a floating branch. The local runners reject
a Spin middleware commit, stable Spin, Wasmtime, `wac`, or `wasm-tools` version
that does not match this lock. Spin support is promoted only after a tagged
release contains final handler, types, and client host support and composes
native middleware against final-WASI WIT.

## Wasmtime composition

Wasmtime serves one already-composed component. It does not dynamically install
middleware. Build or obtain the middleware artifacts, then compose them around
the terminal service before running `wasmtime serve`:

```bash
./scripts/build-middleware-test-components.sh
./scripts/compose-middleware.sh \
  tests/test-app-p3.wasm \
  tests/test-app-p3-middleware.wasm \
  tests/middleware-artifacts/request-id.wasm \
  tests/middleware-artifacts/security-headers.wasm \
  tests/middleware-artifacts/cors.wasm \
  tests/middleware-artifacts/authn-policy.wasm
./scripts/run-middleware-wasmtime.sh
```

Arguments to `compose-middleware.sh` are ordered outermost to innermost after
the application and output paths. The script composes from the application
outward and validates the result.

The local `wasmtime serve` runner enables inherited networking so the authn
component can reach its loopback broker. The stock CLI does not expose a
per-destination HTTP allowlist for a precomposed component. A production
Wasmtime deployment must enforce the exact broker destination in its custom
embedding or an outbound network sandbox; the local CLI runner is not evidence
of that isolation. Native Spin manifests express the narrower broker origin in
`allowed_outbound_hosts`, but remain canaries until Spin supports final WIT.

## Identity propagation

`authn-policy` removes Authorization and every inbound `x-wasi-auth-*` header
before forwarding. It injects one bounded, versioned `x-wasi-auth-context`
value after validating the broker result. In optional mode, missing credentials
produce an explicit anonymous context without calling the broker; supplied but
invalid credentials never fall back to anonymous. The final composed artifact
must be the only externally routable handler, because the terminal service
cannot prove that an inbound metadata header passed through the chain.

`leptos_wasi` already provides `http::request::Parts` to SSR routes and server
functions. Install typed identity or policy context through
`handle_with_context`; that request-context closure runs after the standard
parts are available. That per-request context is the only application-side
trust boundary for middleware identity. Route-discovery context is synthetic,
may be cached, and must never read headers or perform authentication. This does
not require a middleware-specific public API in this crate.

SSR authentication state is presentation-only. It may select navigation,
render a sign-in prompt, or hide a control, but hidden HTML is not an
authorization boundary. Every protected server function must independently
require the authenticated request context and enforce its typed action and
resource policy after deserialization. Route discovery and SSR rendering never
replace `ServerFn::middlewares()` or the equivalent typed authorization call.

The counter and test applications use optional authentication so public SSR and
split-WASM hydration remain available. Protected server functions must require
an authenticated context and authorize their typed action/resource after
deserialization. Whole-service middleware can make coarse method/path decisions
but cannot authorize a resource identifier hidden in a server-function body.
Keep ownership, RBAC, ABAC, and ReBAC decisions in server-function/domain policy
through `ServerFn::middlewares()` or an explicit typed authorization call.

The authorization fixture evaluates its independent Cedar and SpiceDB checks
concurrently. Both decisions remain mandatory; concurrency only removes one
serial provider round-trip from the request critical path.

The companion `leptos-wasi-authz` bridge provides the typed request-context
reader and server-function layers used by the integration fixture. It maps a
missing middleware boundary to 503, an explicit anonymous caller to 401, an
authenticated scope/decision denial to 403, and every malformed or unavailable
provider result to a generic 503. Authentication credentials, cookies, raw
queries, and request bodies are not authorization attributes.

Status ownership is deliberate: missing or invalid authentication returns 401;
an authenticated denial returns 403; broker/PDP transport, malformed data, or
indeterminate policy returns a generic 503. Only an explicit allow reaches the
protected operation.

Do not confuse CORS with CSRF protection. Cookie-authenticated applications
need a separate CSRF design and origin policy.

## Static assets and split WASM

Every Spin HTTP trigger has its own middleware stack. The counter example uses
two terminal services:

- `/...` routes to the Leptos service;
- `/pkg/...` routes to a dedicated file-server component.

Middleware attached to the Leptos trigger does not wrap the file-server
trigger. The experimental example deliberately keeps `/pkg/...` public so
`counter.js`, `counter.wasm`, and lazy `split_*.wasm` modules can hydrate.
Never attach an authentication policy to that asset trigger unless it has an
explicit public bypass.

Response-header or CORS middleware can be added to the asset trigger only when
the file server exports the same compatible WASIp3 HTTP service world. Until
then, apply asset policy at the file server, CDN, or ingress. Do not claim that
the Leptos middleware covers the complete deployment when `/pkg/...` is served
by a separate component.

## Unsupported boundaries

- WASIp2 has no equivalent standard async middleware composition contract.
  Apply policy at ingress or inside the application when Preview 2 support is
  required.
- A WASIp3 HTTP middleware component cannot directly wrap Redis, MQTT, cron, or
  another trigger interface. Those triggers require adapters for their own WIT
  handler worlds, although they may share pure policy code.
- Guest-local counters are not a distributed rate limiter.

## Experimental verification

The integration uses the companion's real request-ID, security, CORS,
authentication, and coarse authorization components plus its deterministic mock
authentication broker, Cedar PDP, and SpiceDB PDP. The tests cover spoof
stripping, anonymous/authenticated context, bearer removal, fail-closed broker
and PDP responses, SSR, server functions, delayed streaming, islands, lazy
split WASM, RBAC/ABAC/ReBAC denials, and sensitive-data log scans. Provider
contract conformance remains owned by the independently versioned `wasi-authz`
workspace.

To localize sustained capacity failures, run
`scripts/benchmark-authz-capacity-matrix.sh`. It compares the default typed
authorization profile with the optional coarse-PEP profile across bounded
offered rates. Set `MIDDLEWARE_DIAGNOSTICS=1` to emit fixed-name stage records
such as `authn_transport`, `coarse_pep_provider`, and `spicedb_upstream`.
Diagnostics never include credentials, identity, cookies, queries, bodies, or
policy attributes and should remain disabled in normal production runs.
The Wasmtime runner also accepts `WASMTIME_MAX_INSTANCE_REUSE_COUNT`,
`WASMTIME_MAX_INSTANCE_CONCURRENT_REUSE_COUNT`, and
`WASMTIME_IDLE_INSTANCE_TIMEOUT` for controlled host-reuse experiments. These
are diagnostic knobs, not substitutes for a deployment-level concurrency
limit or horizontal scaling.

The alpha is not yet promotable. Delayed first-byte delivery, body cancellation,
trailers, and a body that yields one frame before failing now pass through the
composed chain without buffering. The remaining blocker is performance in the
current final-WASI runtime. In a five-pair, 30-second, concurrency-100
representative Leptos workload, a pure pass-through component stayed inside the
10% budget, while the fused secure-defaults component regressed first-byte p99
57.36%, total p99 51.98%, and throughput 29.08%. Policy parsing, full header
copying, and quadratic diffs were removed without materially changing that
result. The two immutable-header request/response reconstructions and their
transmission-result bridges remain the measured hot boundary. Keep the
integration alpha/experimental until the same fixed gate passes; do not weaken
the threshold or infer production readiness from functional E2E alone.

The complete authentication plus Cedar/SpiceDB chain is separately blocked:
its 5,000-request, concurrency-100 run returned 2,317 controlled 503s with
192.975 ms first-byte p99 and 275.921 ms total p99, versus a fixed 25 ms and
zero-failure gate. Its ten-minute soak also failed its request/latency gate,
even though RSS returned below its high-water mark. These local runners accept
only loopback fixture credentials and deliberately refuse production PDP
endpoints or arbitrary secrets because Wasmtime CLI `--env` values are visible
to local process inspection. See [PERFORMANCE.md](./PERFORMANCE.md) for the
retained evidence.

```bash
./scripts/audit-middleware-manifests.py
python3 scripts/test_audit_middleware_manifests.py
python3 scripts/test_verify_artifact_set.py
WASI_HTTP_MIDDLEWARE_BUILD=1 ./scripts/sync-middleware-components.sh
./scripts/run-middleware-tests.sh
MIDDLEWARE=1 HOST=wasmtime ./tests/browser/run.sh
./scripts/run-authz-browser.sh
./scripts/run-authz-lifecycle-e2e.sh
./scripts/run-authz-wasip2-lifecycle-e2e.sh
HOST=spin ./scripts/run-middleware-tests.sh
```

Wasmtime 46 runs the final precomposed chain as the behavioral gate. Stable
Spin's precomposed lane and the exact-commit native middleware lane are
non-blocking incompatibility canaries until Spin publishes tagged final-WASI
host and middleware support.

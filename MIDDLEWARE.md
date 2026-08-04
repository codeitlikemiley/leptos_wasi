# WASIp3 HTTP middleware

`leptos_wasi` is a terminal `wasi:http/service` component. Whole-service
middleware is composed around that terminal component by the deployment; it is
not registered through a `leptos_wasi::Handler` builder method.

The production default keeps request-ID, CORS, response security headers, and
authentication at a trusted private ingress. The ingress removes untrusted
identity headers and forwards one service-bound context to a terminal listener
that is not publicly reachable. The terminal validates and removes that wire
envelope, then installs typed identity in `http::Extensions` before constructing
the `leptos_wasi::Handler`.

One response security header has a per-request input the ingress does not see:
a `Content-Security-Policy` built around the nonce Leptos generates while
rendering. An ingress that does not know that nonce can only send a nonce-free
policy. An application that wants a nonce-based policy emits the header from
the terminal component through `ResponseOptions`; see [Content Security
Policy](./README.md#content-security-policy). The remaining response security
headers stay with the ingress components.

Reusable WASIp3 components remain available as a portable experimental mode.
The same artifact can wrap other WASIp3 HTTP services, while each HTTP trigger
selects its own ordered stack.
The companion `wasi-http-middleware` repository implements that reusable chain
as independently compiled components. Local integration runners verify and
copy checksum-pinned artifacts from the sibling checkout; `leptos_wasi` does
not carry a second protocol-only implementation.

The reference production topology is executable through
`scripts/run-trusted-ingress-browser.sh`. It exposes only the native ingress;
Wasmtime binds to a private listener. The context envelope is bounded and
encoded, but deliberately not signed. Production deployments therefore must
use private networking or mutually authenticated TLS between ingress and
terminal. Guest-held signing keys are not a substitute for that boundary.

## Scope and ordering

There are three distinct policy boundaries:

1. **WASIp3 component middleware** applies to the complete selected HTTP
   service. Use it for framework-neutral request and response policy.
2. **Leptos server-function middleware** registered through
   `with_server_fn::<T>()` applies to one typed server function. Use it for
   authorization or validation that depends on that endpoint.
3. **Host or ingress policy** owns TLS, WAF rules, trusted client addresses,
   request deadlines, concurrency, memory limits, and distributed rate limits.

The portable component stack is:

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
Tagged Spin 4.0.2 rejects that component because its host linker does not
provide the final `wasi:http/types@0.3.0` resource implementation. The pinned
Spin main revision runs plain final-WASI terminals and outbound HTTP, but its
default CPU accounting hook panics when a WAC-composed handler is invoked.
Building that revision without default features proves the component chain but
is diagnostic only. Wasmtime 46.0.1 is therefore the only blocking final-WASI
behavioral runtime in this release.
Production deployments must consume versioned artifacts pinned by digest. The
local runner accepts a sibling checkout only after checking the declared
compatibility tuple, artifact checksums, component WIT, SBOMs, provenance
subjects, OCI manifest, public key, and detached signature bundles. The sync
gate runs pinned Cosign verification for both the provenance statement and OCI
manifest before copying a component. The checked
`tests/middleware/artifact-sets.toml` record binds those exact files.

Both bundles are recorded from published releases, each on its own version line
and its own tag: `wasi-authz 0.1.0-rc.3` from
[`wasi-auth-v0.1.0-rc.3`](https://github.com/codeitlikemiley/wasi-auth/releases/tag/wasi-auth-v0.1.0-rc.3),
and `wasi-http-middleware 0.2.0-alpha.3` from
[`wasi-http-middleware-v0.2.0-alpha.3`](https://github.com/codeitlikemiley/wasi-auth/releases/tag/wasi-http-middleware-v0.2.0-alpha.3).
Each is rebuilt and signed by a tag-triggered release workflow that refuses any
tag whose commit is not on `main` or whose name does not match the prepared
version.

Neither may be regenerated locally. The signing key is generated per release run
and discarded, so `signing_key_sha256`, `provenance_signature_sha256`, and
`manifest_signature_sha256` can only ever be satisfied by the one immutable
bundle they were recorded from. Re-verify by re-downloading the assets. The
component digests are equally unreproducible off the release lane, because the
component build embeds absolute source paths and so depends on the checkout
location; the SBOM and WIT digests are not, and hold across rebuilds and
platforms.

`AUTHZ_COMPANION_ALLOW_DIRTY=1` still bypasses the authorization name/version
comparison for explicit local diagnostics. Note what that costs: it also skips
signed artifact-set verification entirely, and a lane that rebuilds the
components as part of its own setup cannot satisfy the release checksum
manifest afterwards.

The exact experimental runtime, SDK, WIT, and composition-tool revisions are
recorded in
[`tests/middleware/components.lock.toml`](tests/middleware/components.lock.toml).
Do not replace those revisions with a floating branch. The local runners reject
a Spin middleware commit, stable Spin, Wasmtime, `wac`, or `wasm-tools` version
that does not match this lock. Spin support is promoted only after a tagged
release contains final handler, types, and client host support, fixes composed
handler CPU accounting, and composes native middleware against final-WASI WIT.

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

In the production profile, `wasi-http-authn::accept_trusted_ingress` validates
service ID, audience, lifetime, duplicate metadata, and the absence of a
surviving bearer token. It strips the wire envelope and installs
`VerifiedAuthContext` in `http::Extensions`. Missing typed context is a bypass,
not anonymous identity, and maps to 503. The portable component profile must
perform the same explicit promotion before `Handler::build`.

`leptos_wasi` provides the resulting `http::request::Parts`, including
extensions, to SSR routes and server functions. `handle_with_context` installs
the typed Leptos context after those standard parts are available. Route
discovery remains synthetic and must never perform authentication.

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

The authorization fixture embeds and initializes Cedar once. RBAC and ABAC are
evaluated locally; only relationship-sensitive operations call SpiceDB. Hybrid
operations evaluate Cedar first and never call SpiceDB after a Cedar denial.

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

The portable component alpha is not yet promotable. Delayed first-byte delivery, body cancellation,
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

The current pinned artifact set removes that earlier admission failure at the
tested configuration: a fresh 1,000-request, 100-concurrency domain-only probe
returned zero failures, but first-byte and total p99 remained 73.841 ms and
74.258 ms. The documented Wasmtime reuse tuning lowered those to 67.650 ms and
68.921 ms, still above the 25 ms promotion target. Treat this as an active
capacity blocker, not as evidence that the full chain is production-ready.

Those portable guest-component measurements are not promotion evidence for
the trusted-ingress topology. The native ingress avoids guest response-header
reconstruction and keeps Cedar embedded in the terminal; SpiceDB is the only
authorization network hop for relationship checks. Generate promotion evidence
from release builds with `scripts/run-trusted-ingress-browser.sh`,
`scripts/benchmark-trusted-ingress.sh`, and
`DURATION=600 CONCURRENCY=100 scripts/soak-trusted-ingress.sh`.

The production fixture calls SpiceDB directly from the terminal. The remote
AuthZEN SpiceDB PDP is retained only as an explicitly selected compatibility
profile and is not started by the trusted-ingress runner. The benchmark records
paired proxy/edge runs plus authentication-only, embedded Cedar, direct
SpiceDB, Cedar-first hybrid, and Cedar-denial profiles. `trusted-load` uses
persistent Hyper connections and accounts for every success, status failure,
transport failure, cancellation, and hung request. The old Python soak client
is not promotion evidence.

The soak runner stores `result.json`, ingress `diagnostics.json`, and a
machine-readable `summary.json` under `AUTHZ_FULL_CHAIN_SOAK_DIR` (default
`target/authz-full-chain-soak`). The summary gate rejects a truncated run,
unexpected status or transport outcome, cancellation or hang, any p99 above
25 ms, a process missing at the final sample, or per-process final-quarter RSS
growth above `max(32 MiB, 10%)`.

Ingress admission is bounded by route class and holds permits until the body
finishes or is dropped. Healthy terminal replicas are chosen by least active
load with round-robin tie breaking. This backpressure protects the topology;
it does not create capacity, and any overload 503 fails promotion. Route policy
is deployment data in `tests/trusted-ingress/routes.toml`. Wasmtime lifecycle
values must be selected from measured repeated runs with
`scripts/tune-trusted-ingress.sh`, not copied between hosts as universal
defaults. The soak applies a final-quarter RSS growth limit of
`max(32 MiB, 10%)`. Local alpha signatures use ephemeral development keys only.
Published OCI artifacts require approved CI keyless signing or a durable
release identity.

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
./scripts/run-trusted-ingress-browser.sh
./scripts/run-trusted-ingress-spin-main.sh
./scripts/benchmark-trusted-ingress.sh
DURATION=600 CONCURRENCY=100 ./scripts/soak-trusted-ingress.sh
HOST=spin ./scripts/run-middleware-tests.sh
```

Wasmtime 46.0.1 runs the final precomposed chain as the behavioral gate. Tagged
Spin's linker lane, pinned Spin main's CPU-metrics regression lane, and the
exact-commit native middleware lane remain incompatibility canaries until Spin
publishes tagged final-WASI host and middleware support.

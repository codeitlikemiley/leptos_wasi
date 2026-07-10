# WASIp3 HTTP middleware

`leptos_wasi` is a terminal `wasi:http/service` component. Whole-service
middleware is composed around that terminal component by the deployment; it is
not registered through a `leptos_wasi::Handler` builder method.

This keeps reusable authentication, CORS, request-ID, and response-header
components independent of Leptos. The same middleware artifact can wrap other
WASIp3 HTTP services, while each HTTP trigger selects its own ordered stack.
The companion `wasi-http-middleware` repository implements that reusable chain
as independently compiled `0.1.0-alpha.1` components. This repository keeps a
smaller protocol fixture so its compatibility lane does not depend on an
unpublished sibling checkout.

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
request-id -> security-headers -> cors -> auth-policy -> leptos service
```

Requests travel from left to right and responses travel from right to left.
This ordering lets CORS, request-ID, and security-header middleware decorate an
authentication rejection without invoking the application.

Middleware components are composed in-process. They are separate WebAssembly
components, not separately deployed HTTP proxies, so the normal design does not
add a network hop.

## Spin composition

Spin's middleware manifest support is currently a vNext feature. The
experimental manifest in
[`tests/spin-p3-middleware-vnext.toml`](tests/spin-p3-middleware-vnext.toml)
uses the following shape:

```toml
[[trigger.http]]
route = "/..."
component = "leptos-app"
dependencies.middleware = [
  { component = "request-policy" },
]
```

The dependency list is ordered outermost to innermost. Production deployments
should consume versioned registry packages or URL artifacts pinned by digest.
The checked-in fixture uses a local component only because it is a protocol
compatibility test.

The exact experimental runtime, SDK, WIT, and composition-tool revisions are
recorded in
[`tests/middleware/components.lock.toml`](tests/middleware/components.lock.toml).
Do not replace those revisions with a floating branch. This integration does
not become supported until a stable Spin release uses a WIT revision compatible
with the application's `wasip3` bindings. The local runners reject a Spin,
Wasmtime, `wac`, or `wasm-tools` version that does not match this lock.

## Wasmtime composition

Wasmtime serves one already-composed component. It does not dynamically install
middleware. Build or obtain the middleware artifacts, then compose them around
the terminal service before running `wasmtime serve`:

```bash
./scripts/build-middleware-test-components.sh
./scripts/compose-middleware.sh \
  tests/test-app-p3.wasm \
  tests/test-app-p3-middleware.wasm \
  tests/middleware-fixture.wasm
./scripts/run-middleware-wasmtime.sh
```

Arguments to `compose-middleware.sh` are ordered outermost to innermost after
the application and output paths. The script composes from the application
outward and validates the result.

## Identity propagation

An authentication component should remove every inbound copy of its trusted
identity headers before validating credentials. It may add trusted identity
metadata only after successful validation. The final composed artifact must be
the only externally routable handler; exposing the unwrapped terminal service
would let callers forge those headers.

`leptos_wasi` already provides `http::request::Parts` to SSR routes and server
functions, so applications can read middleware-injected headers from that
standard request context. This does not require a middleware-specific public
API in this crate.

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

The local fixture adds one request header before forwarding and one response
header after the Leptos service returns. For browser-contract coverage it also
rejects unauthenticated `/api/...` requests and accepts the deterministic
`Bearer allow` test credential. It is intentionally not a production identity
component. The companion repository owns the external-policy implementation,
strict path normalization, spoof stripping, fail-closed errors, concurrency,
disconnect, slow-provider cancellation, and log-secrecy tests.

The current RC SDK adapter rebuilds forwarded requests, so this fixture removes
`host` and hop-by-hop fields that a new WASI fields resource forbids. Its smoke
test covers successful delayed streaming. Deliberate stream-failure and upload
disconnect behavior remains authoritative in the direct `wit-bindgen`
companion chain until the Spin SDK bridge reaches a matching stable release.

```bash
./scripts/audit-middleware-manifests.py
./scripts/run-middleware-tests.sh
MIDDLEWARE=1 HOST=wasmtime ./tests/browser/run.sh
MIDDLEWARE=1 HOST=spin ./tests/browser/run.sh
```

The ordinary stable-runtime test and browser jobs remain the production gates.
The vNext middleware CI job is non-blocking until Spin publishes stable support.

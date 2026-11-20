# leptos_wasi security model

## System overview
`leptos_wasi` (crates.io `leptos-wasi-runtime`) is a library. Apps embed it as their `wasi:http` terminal handler on Wasmtime or Spin, under WASI Preview 2 (`wasip2`) or Preview 3 (`wasip3`). It converts incoming WASI requests to `http` types, enforces body-size policy, dispatches typed Leptos server functions, renders SSR/islands routes (with optional CSP nonce), serves an optional static-file callback, and streams responses back to the host.

Trust boundaries:
- **Internet client → host ingress → guest component.** Every byte of method, path, query, headers and body reaching `Handler` is attacker-controlled.
- **Host (Wasmtime/Spin) and ingress.** Trusted. They own request deadlines, concurrency caps and filesystem preopens (PRODUCTION.md).
- **Middleware chain → terminal.** Optional WASIp3 middleware (`authn-policy`, CORS, security headers) wraps the terminal. `leptos_wasi` does not parse `x-wasi-auth-context`. It only forwards `http::Extensions` installed before `Handler::build` (MIDDLEWARE.md "Identity propagation").
- **Library → application code.** Server functions and static callbacks are app code. Per-resource authorization lives there, not in the library.

## Components
| Name | Code |
|---|---|
| Request ingestion | `src/request.rs`, `src/handler/policy.rs`, `src/handler/http_util.rs` |
| Static files | `src/static_files.rs`, static routing in `src/handler/core.rs` |
| Server functions | `src/handler/server_fns.rs`, `src/handler/builder.rs` |
| SSR & routing | `src/handler/render.rs`, `src/handler/routes.rs`, `src/integration.rs` |
| Response & streaming | `src/response.rs`, `src/utils.rs` |
| Runtime adapters | `src/handler/wasip2.rs`, `src/handler/wasip3.rs`, `src/executor/**` |
| Tracing | `src/handler/trace.rs` |

## Entry points and who can reach them
- The `wasi:http` incoming handler (P2 `IncomingRequest`/`ResponseOutparam`, P3 `Request`). Anyone on the internet can reach it through the app's ingress.
- Server-function endpoints (POST, e.g. `/api/*`). Their bodies are deserialized by `server_fn`. Form POSTs trigger the redirect path, which uses `Referer` and `Location` sanitization (`apply_server_fn_redirect`, `sanitize_referrer`).
- SSR routes (GET). These cover streaming, async, out-of-order and islands-router rendering.
- Static callback routes (GET/HEAD). The path goes through `normalize_static_path`.
- `x-request-id` is read into tracing spans.

## Assets
- Availability of the guest instance: memory, executor queue, body buffering up to `DEFAULT_MAX_REQUEST_BODY_SIZE` (16 MiB).
- Integrity of the app's authorization context: `http::Extensions` and request `Parts` passed to server functions.
- Files under the static asset root. Nothing outside it must be readable.
- Users' browsers: SSR HTML/script injection, CSP nonce secrecy and per-request uniqueness, open redirects via `Location`.
- Logs: no bodies, auth headers, cookies or raw queries.

## Attacker profiles
- **Anonymous internet client** (user-confirmed: the attacker that matters is anyone on the internet hitting an app built on leptos_wasi). They can send arbitrary requests to every entry point above.

## Existing controls
- Content-Length validation (400 on conflicting values, 413 over the limit) and collected-size checks on both previews.
- Optional whole-body read timeout (`with_request_body_timeout_ns`).
- Static path normalization: one percent-decode, then rejection of encoded separators, residual encoding, NUL/control bytes, absolute paths and drive prefixes, and `.`/`..`. Only GET/HEAD are served, with `nosniff`.
- The server-fn `Location` is reduced to a path, and the referrer is sanitized.
- The CSP nonce is minted per request.
- `unsafe_code = "forbid"`, and `cargo deny` runs on every release.

## Out of scope / accepted risk
- **User-confirmed:** `examples/`, `tests/` (fixtures, authz/middleware test components) and `scripts/` are out of scope entirely. The scan excludes them.
- From the repo docs (PRODUCTION.md, MIDDLEWARE.md), not yet confirmed by the user:
  - Request read deadlines and concurrency caps are a host/ingress responsibility.
  - Symlink containment is the static callback's job.
  - Authn/authz is middleware and server-function policy, not the library's.
  - CSRF for cookie-auth apps is the app's design.

## Infrastructure
No infrastructure repository (user-confirmed). Deployment is described only by the in-repo Spin manifests (`examples/counter/spin*.toml`, `tests/spin*.toml`).

## Open questions
- Should the documented deployment responsibilities above be confirmed as accepted risk?

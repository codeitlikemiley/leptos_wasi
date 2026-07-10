# Leptos authorization fixture

This nested crate is the local cross-repository consumer for
`leptos-wasi-authz`. It is intentionally outside the root package so an
unpublished sibling dependency cannot break standalone `leptos_wasi` builds.

The fixture reuses the counter's SSR/islands application and browser wire type,
but registers a protected server implementation at the same
`/api/increment_count` path. The outer component middleware authenticates the
request; `RequireAuthLayer` rejects an anonymous caller before invocation, and
the deserialized operation calls environment-configured Cedar and SpiceDB
AuthZEN PDPs over final-WASIp3 HTTP clients with a typed `counter.increment`
action and `counter/session-counter` resource. Cedar enforces RBAC/ABAC first;
SpiceDB enforces the relationship immediately before mutation. Provider denial
maps to 403, provider failure maps to 503, and only two explicit allows
increment the counter.

Run it only through `scripts/check-authz-companion.sh` and the local composed
browser runner. Both sibling repositories must match the full revisions in
`tests/middleware/components.lock.toml` and have clean worktrees.

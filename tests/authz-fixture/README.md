# Leptos authorization fixture

This nested crate is the local cross-repository consumer for
`leptos-wasi-authz`. It is intentionally outside the root package so an
unpublished sibling dependency cannot break standalone `leptos_wasi` builds.

The fixture reuses the counter's SSR/islands application and browser wire type,
but registers a protected server implementation at the same
`/api/increment_count` path. The trusted ingress authenticates the request and
the terminal explicitly promotes its validated wire envelope into a typed
request extension before `Handler::build`. `RequireAuthLayer` rejects an
anonymous caller before invocation. The deserialized operation evaluates an
embedded Cedar provider for RBAC/ABAC and calls the environment-configured
SpiceDB AuthZEN PDP only for the relationship check immediately before
mutation. Provider denial maps to 403, provider failure maps to 503, and only
explicit allows increment the counter.

Run it only through `scripts/check-authz-companion.sh` and the local composed
browser runner. Both sibling repositories must match the full revisions in
`tests/middleware/components.lock.toml` and have clean worktrees.

# Spin final-WASI compatibility

The checked runtime contract separates four behaviors that must not be
collapsed into one "Spin support" flag.

| Profile | Expected result |
|---|---|
| Tagged Spin 4.0.2 | Rejects final `wasi:http/types@0.3.0` |
| Pinned Spin `4.1.0-pre0` (`c34c584...`) terminal | Serves SSR, server functions, static callbacks, islands, split WASM, and the SQLite counter client |
| Pinned Spin main trusted terminal | Passes native-ingress authentication plus Cedar and direct SpiceDB authorization |
| Pinned Spin main WAC composition | Default build panics in CPU-time call-hook accounting |
| Pinned Spin main without default features | Composed chain passes, but is diagnostic only |
| Native `dependencies.middleware` revision | Requests the March RC handler and rejects final middleware |

The exact main revision is recorded in
`tests/middleware/components.lock.toml`. It is an experimental compatibility
input, not a floating branch or production support claim.

## Reproduce

```bash
./scripts/bootstrap-spin-main.sh
SPIN_BIN=target/tools/spin-*/spin ./scripts/check-spin-main-terminal.sh
SPIN_BIN=target/tools/spin-*/spin ./scripts/check-spin-main-composed-canary.sh
```

The composed canary wraps the terminal with a pass-through-compatible final
WASI handler. The first request reaches Spin's `CpuTimeCallHook`, where the
current implementation assumes call-hook transitions cannot nest and unwraps
an empty timestamp. Disabling default features removes the CPU hook and proves
that the component ABI and chain are otherwise runnable.

The upstream fix must retain CPU metrics. It should add a composed asynchronous
handler regression test, replace the single timestamp assumption with a
nesting-aware transition state, and verify monotonic CPU time across host
calls, nested guest calls, cancellation, and stream failure. Ignoring unmatched
events without validating metric accuracy is not sufficient.

Native middleware is a separate upstream issue. Before contributing another
change, coordinate with the maintainers on the work that superseded Spin PR
`#3602`. Any patch must include final-WIT composition, capability inheritance,
configuration isolation, outbound HTTP, streaming, cancellation, and ordering
tests rather than only changing the interface string.

## Promotion

Production remains native trusted ingress plus a plain terminal. Stable Spin
support requires a tagged release containing final terminal/outbound HTTP and
the CPU-accounting fix. Stable native middleware additionally requires final
`dependencies.middleware` support. The no-default-features build never counts
as release or soak evidence.

# Spin final-WASI compatibility

The checked runtime contract separates four behaviors that must not be
collapsed into one "Spin support" flag.

| Profile | Expected result |
|---|---|
| Tagged Spin 4.0.2 | Rejects final `wasi:http/types@0.3.0`. Reproduced on 2026-10-24 against this revision's Preview 3 test-app: `resource implementation is missing` |
| Official Spin 4.2.5 (`e09d7d5`, 2026-10-23) | Serves the test-app Preview 2 and Preview 3 guests, including `POST /api/post_test` and `GET /ssr/async`, on Wasmtime-equivalent assertions. Also serves a WAC plug and native `dependencies.middleware` of `request-id`, `security-headers`, and `cors` around the `wasip3` 0.9 terminal. Browser islands, SQLite, `authn-policy`, and HTTP/1.1 trailers were not run. This row is not the CI install |
| Pinned Spin `4.1.0-pre0` (`c34c584...`) terminal | Serves SSR, server functions, static callbacks, islands, split WASM, and the SQLite counter client |
| Pinned Spin main trusted terminal | Passes native-ingress authentication plus Cedar and direct SpiceDB authorization |
| Pinned Spin main WAC composition | Default build panics in CPU-time call-hook accounting |
| Pinned Spin main without default features | Composed chain passes, but is diagnostic only |
| Native `dependencies.middleware` revision | Requests the March RC handler and rejects final middleware |

The exact main revision is recorded in
`tests/middleware/components.lock.toml`. It is an experimental compatibility
input, not a floating branch or production support claim.

Final-WASI describes the exported component world, not the Rust codegen target.
Until Rust distributes self-contained libraries for `wasm32-wasip3`, release
artifacts use the supported `wasm32-wasip2` Rust target and must be inspected
with `wasm-tools component wit`: they must export
`wasi:http/handler@0.3.0` and must not retain `wasi_snapshot_preview1`
imports. The `wasm32-wasip3 -Zbuild-std` lane is a canary only.

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

Production remains native trusted ingress plus a plain terminal. Spin 4.2.5
links final `wasi:http@0.3.0` for the test-app terminal and for the public
middleware stack recorded in [COMPATIBILITY.md](./COMPATIBILITY.md). The
`c34c584` CPU-accounting panic and the March RC native-middleware canary were
not re-run on 4.2.5, so they stay expected-failure lanes for those older
hosts. The no-default-features build never counts as release or soak evidence.
CI still installs Spin 4.0.2 for the companion canary.

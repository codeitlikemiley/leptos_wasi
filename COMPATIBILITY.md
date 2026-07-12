# Tested compatibility

This is the reproducible tool and ABI tuple for the current `0.4.2-alpha.3`
development line. The machine-readable source is
[`tests/middleware/components.lock.toml`](./tests/middleware/components.lock.toml).

| Layer | Tested version | Contract |
|---|---|---|
| Rust MSRV | 1.93.0 | Library and example minimum |
| Rust targets | `wasm32-wasip2`, `wasm32-unknown-unknown` | WASI server and browser hydration |
| `wasip3` crate | 0.7.0 | Final `wasi:http@0.3.0` bindings |
| Cargo Leptos | 0.3.6 | Islands and lazy WASM splitting |
| `wasm-bindgen` crate/CLI | 0.2.126 | Browser artifacts; crate and CLI must match |
| Wasmtime | 46.0.1 | Blocking final-WASI correctness reference |
| Tagged Spin | 4.0.2 | Preview 2 supported; final WASI 0.3 expected to fail |
| Experimental Spin | 4.1.0-pre0 at `c34c584dbf77b3a3528ad0536aa9ce4761b9f772` | Final terminal and outbound HTTP compatibility |
| Spin SDK | 6.0.0 | Component fixture SDK |
| `wit-bindgen` | 0.59.0 | Middleware component generation |
| `wasm-tools` | 1.253.0 | Component inspection and composition support |
| WAC | 0.10.1 | Deterministic precomposition |

Install the developer-facing tools:

```bash
rustup target add wasm32-wasip2 wasm32-unknown-unknown
cargo install cargo-leptos --version 0.3.6 --locked
cargo install wasm-bindgen-cli --version 0.2.126 --locked
cargo install wasmtime-cli --version 46.0.1 --locked
cargo install --git https://github.com/spinframework/spin \
  --rev c34c584dbf77b3a3528ad0536aa9ce4761b9f772 \
  --locked --force spin-cli
```

Validate the local counter toolchain before building:

```bash
./scripts/check-counter-toolchain.sh wasmtime
./scripts/check-counter-toolchain.sh spin
```

## `wasm-bindgen` WASI regression

Version 0.2.114 worked for WASI because browser placeholder imports were not
emitted on `target_os = "wasi"`. Version 0.2.115 broadened the gate to the
entire WebAssembly target family, causing WASIp2 components to retain unresolved
`__wbindgen_placeholder__` imports. The fix was merged in
[wasm-bindgen #5175](https://github.com/wasm-bindgen/wasm-bindgen/pull/5175)
and released in 0.2.123. The current tested version is 0.2.126, so the old
0.2.114 workaround is no longer required.

The browser CLI and the `wasm-bindgen` crate resolved in the counter lockfile
must remain identical. This version does not define the server ABI: the server
component uses `wasip3` 0.7.0 and final `wasi:http@0.3.0`.

The temporary 0.2.114 pin and its Leptos template context are recorded in
[leptos-spin #63](https://github.com/spinframework/leptos-spin/pull/63).

## Runtime claims

Wasmtime 46.0.1 is the correctness reference, not a claim that its stock CLI
meets the authorization latency SLO. Tagged Spin 4.0.2 lacks final WASI 0.3
resource implementations. The pinned Spin main commit runs the plain terminal,
outbound HTTP, SQLite persistence, SSR, islands, and split WASM, but remains an
experimental input until that support ships in a tagged release.

See [Spin final-WASI compatibility](./SPIN_COMPATIBILITY.md) for the remaining
WAC CPU-accounting and native middleware limitations.

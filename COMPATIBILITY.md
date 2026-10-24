# Tested compatibility

Two contracts are in force. They are not the same tuple.

The package version stays `leptos-wasi-runtime` 0.4.2. crates.io 0.4.2
(2026-08-05) is older than git `6727a21` and still declares `wasip3` 0.7.0.
Downstream guests that need `wasip3` 0.9 should git-depend on this revision.
There is no 0.5.0-alpha.3 release of this crate.

## Library pins on this revision

| Layer | Version | Contract |
|---|---|---|
| `rust-version` | 1.94.0 | `Cargo.toml`. `AtomicU64::try_update` is not used |
| Toolchain | 1.99.0 (`b940084d7`, 2026-09-28) | `rust-toolchain.toml` |
| Rust targets | `wasm32-wasip2`, `wasm32-unknown-unknown` | WASI server and browser hydration |
| `wasip3` | `=0.9.0` (`0.9.0+wasi-0.3.0`) | Final `wasi:http/handler@0.3.0`. `spawn` and `spawn_local` both call `wasip3::spawn_local` |
| `wit-bindgen` | 0.62.0 | Pulled by `wasip3` 0.9. `tests/authz-lifecycle` pins `=0.62.0`. The root lock also still contains 0.57.1 from the WASIp2 edge |
| `leptos` / `leptos_macro` | 0.8.22 / `=0.8.19` | Stay off `leptos` 0.9.0-rc0 |
| `server_fn` | 0.8.13 | Lock holds `server_fn_macro_default` 0.8.6 (`syn` 3). 0.8.5 is `syn` 2 and does not parse |
| `wasm-bindgen` | 0.2.129 | Counter pin and root lock (`wasm-bindgen-futures` 0.4.79, `js-sys` / `web-sys` 0.3.106) |
| Guest `wasip2` | 1.0.3+wasi-0.2.9 | `tests/test-app` pin, so the component exports `wasi:http/incoming-handler@0.2.9`. An unpinned update floated the world to 0.2.12 |

`production-counter/store` does not use `wasip3` and keeps `rust-version` 1.93.0.

## Signed companion lock

[`tests/middleware/components.lock.toml`](./tests/middleware/components.lock.toml)
is the checksum-pinned `wasi-auth` 0.1.0-rc.3 / legacy middleware
`0.2.0-alpha.3` companion. It still records Rust 1.93.0, `wasip3` 0.7.0,
`wasm-bindgen` 0.2.126, Wasmtime 46.0.1, tagged Spin 4.0.2,
`wit-bindgen` 0.59.0, Spin SDK 6.0.0, `wasm-tools` 1.253.0, WAC 0.10.1, and
the Spin fork `4.1.0-pre0` at `c34c584dbf77b3a3528ad0536aa9ce4761b9f772`.
`spin_final_wasi_supported` stays false. The failure string stays
`wasi:http/types@0.3.0 resource implementation is missing`.

`scripts/sync-middleware-components.sh` compares that lock to the companion
checkout. CI middleware jobs still install Wasmtime 46.0.1 and Spin 4.0.2.
The library `wasip3` pin is not that lock's `wasip3_version`. Moving the lock
requires a companion release that is not this crate's change.

`tests/authz-fixture` and `tests/authz-lifecycle` still path-depend on that
0.7 companion, so their lockfiles contain both `wasip3` 0.7.0 and 0.9.0.
Those fixtures were not recompiled on this revision.

## Hosts verified on 2026-10-24

This is an additional executed tuple. It does not replace the companion lock
or the CI install steps.

| Tool | What ran | Result |
|---|---|---|
| rustc 1.99.0 | `cargo +1.99.0` fmt, clippy `-D warnings`, test, doc for default, `wasip2,islands-router`, `wasip3,islands-router`, both, and all-features | Passed. Lib tests 152 / 153 / 129 / 164 / 165. Ignored e2e tests stayed ignored |
| rustc 1.94.0 (`4a4ef493e`, 2026-03-02) | test, clippy, doc for `wasip2,islands-router`, `wasip3,islands-router`, and both. `CARGO_TARGET_DIR=/tmp/tgt-194` | Passed. Lib tests 153 / 129 / 164 |
| Wasmtime 50.0.0 (`ebe5e8df1`, 2026-10-20), `/tmp/tools/wasmtime50/wasmtime` | `cargo +1.99.0 test --locked --test e2e -- --ignored --test-threads=1` with `WASMTIME_BIN` set, filters `test_e2e_wasip2` and `test_e2e_wasip3` | Passed, including `POST /api/post_test`, SSR, a 64 KiB body, chunked overflow, a slow upload, and client disconnect |
| Spin 4.2.5 (`e09d7d5`, 2026-10-23), official linux-amd64 `/tmp/tools/spin425/spin` | Same e2e command with `SPIN_BIN`, filters `test_e2e_spin_wasip2` and `test_e2e_spin_wasip3` | Passed. Same body cases |
| Spin 4.0.2 `/tmp/tools/spin402/spin` | `spin up` of the current Preview 3 guest | Still fails: `wasi:http/types@0.3.0` resource implementation is missing |
| `wasi-http-middleware` `896f8f5426b1bdc6d208367d0ac8dbf44146dfda` | WAC 0.10.1 plug of `cors`, then `security-headers`, then `request-id` around the clean `wasip3` 0.9 test-app. `wasm-tools` 1.253.0 `validate --features component-model,cm-async`. Export is only `wasi:http/handler@0.3.0` | See the composition note below |

### Body-consuming routes

`Handler::build` always `await`s `http_body::Body::collect` (`src/handler/wasip3.rs` `ingest`). `wasip3` 0.7.0 and 0.9.0 `IncomingBody::poll_frame` both `await` `stream.read`. That path does not call a synchronous wait on a waitable that is already in a set.

Spin 4.2.5 served a clean release guest (`wasm32-wasip2`, `--no-default-features --features wasip3,islands-router`) built with rustc 1.99.0:

| Guest | `GET /api/get_test` | `GET /ssr/async` | `POST /api/post_test` `{"msg":"hello"}` | same POST with a 60 KiB `msg` |
|---|---|---|---|---|
| git `6727a21`, `wasip3` `=0.7.0`, `wit-bindgen` 0.57.1 | 200 `"GET response"` | 200 HTML | 200 `"POST response: hello"` | 200, body echoed |
| this revision, `wasip3` `=0.9.0`, `wit-bindgen` 0.62.0 | 200 `"GET response"` | 200 HTML | 200 `"POST response: hello"` | 200, body echoed |

Guest stderr on both is the existing `LEPTOS_OUTPUT_NAME` warning. Neither log contains `trap` or `waitable`.

Agent 7 root-caused that trap to wit-bindgen 0.57.1 `inter-task-wakeup`: a synchronous `stream.cancel-read` while the waitable is still in a waitable set (`FutureState::callback` during `wasi:http/handler@0.3.0#handle`). The fix is wit-bindgen 0.59+ ([wit-bindgen #1638](https://github.com/bytecodealliance/wit-bindgen/pull/1638)). This crate's body collector is not the cause. `wasip3` 0.9.0 depends on `wit-bindgen` 0.62.0, so this revision no longer pulls the 0.57.1 bindings. The old single-file `src/executor.rs` `wasip3::spawn` calls (0.4.2-rc.1 lines 1003 and 1007) now live in `src/executor/p3.rs`, and both `spawn` and `spawn_local` call `wasip3::spawn_local`. Direct `wasip3 =0.7.0` pins are gone from this crate and its in-repo guests. The root `Cargo.lock` resolves only `wasip3` 0.9.0+wasi-0.3.0. `tests/authz-fixture` and `tests/authz-lifecycle` still also lock 0.7.0 through the `wasi-auth` 0.1.0-rc.3 path dependency; that is the companion, not the runtime crate a fullstack guest links.

### Composition

The public stack (no `authn-policy`) was plugged onto the clean 0.9 terminal. CORS was configured with origin `https://public.example`, methods `GET,HEAD,POST,OPTIONS`, header `content-type`, credentials off.

With those variables set, Wasmtime 50 (`--env`), Spin 4.2.5 WAC-composed, and Spin 4.2.5 `dependencies.middleware` (order `request-id`, `security-headers`, `cors`) each returned:

- `GET /` 404 HTML (no route at `/`)
- `GET /ssr/async` 200 HTML
- `POST /api/post_test` 200 `"POST response: hello"`

Each response had one `x-content-type-options: nosniff`, one `referrer-policy: strict-origin-when-cross-origin`, one `x-request-id`, and `access-control-allow-origin: https://public.example`. No `trap` or panic.

Wasmtime 50 serve of the same composed component without the CORS environment variables returned 503, while `x-content-type-options`, `referrer-policy`, and `x-request-id` were still present. HTTP/1.1 response trailers were not tested. `authn-policy` was not in this chain. The native-middleware canary script that targets Spin revision `27451471` was not re-run. The fork at `c34c584` CPU-hook panic was not re-run.

## Not re-run

Browser E2E, cargo-leptos islands build, ten-minute soaks, benchmarks, identity tests against `wasi-auth` newer than 0.1.0-rc.3, and the 0.4.2-rc.1 test-app binary. CI still installs cargo-leptos 0.3.6. `.cursor/install.sh` installs cargo-leptos 0.3.7, Wasmtime 50.0.0, and `wasm-bindgen` 0.2.129 for the library toolchain. That script is not the companion lock.

Install the developer-facing tools:

```bash
rustup target add wasm32-wasip2 wasm32-unknown-unknown
cargo install cargo-leptos --version 0.3.7 --locked
cargo install wasm-bindgen-cli --version 0.2.126 --locked
cargo install wasmtime-cli --version 46.0.1 --locked
cargo install --git https://github.com/codeitlikemiley/spin \
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
and released in 0.2.123. The counter on this revision locks 0.2.129. The
companion lock still records 0.2.126 for the signed middleware checkout. The
0.2.114 workaround is not required for either.

The browser CLI and the `wasm-bindgen` crate resolved in the counter lockfile
must remain identical. This version does not define the server ABI: the server
component uses `wasip3` 0.9.0 and final `wasi:http@0.3.0`.

The temporary 0.2.114 pin and its Leptos template context are recorded in
[leptos-spin #63](https://github.com/spinframework/leptos-spin/pull/63).

## Runtime claims

The companion lock's Wasmtime 46.0.1 lane remains the CI correctness reference
for the signed middleware checkout. It is not a claim that its stock CLI meets
the authorization latency SLO. Tagged Spin 4.0.2 lacks final WASI 0.3 resource
implementations; that rejection was reproduced against this revision's Preview 3
guest. The pinned Spin main commit remains an experimental input in the lock.
Official Spin 4.2.5 does link and serve this revision's Preview 3 test-app,
including body-consuming routes and the public middleware stack above. That
does not promote browser islands, SQLite, soaks, or `authn-policy`.

See [Spin final-WASI compatibility](./SPIN_COMPATIBILITY.md).

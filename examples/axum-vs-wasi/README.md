# axum vs leptos_wasi

Same Leptos 0.8 app, two servers:

- **wasi**: `leptos_wasi` Preview 2 on Wasmtime, with a once-per-instance
  `RouteTable` installed via `generate_routes_from`
- **axum**: `leptos_axum` SSR on tokio

This folder is a measurement harness, not a stored benchmark. Run it on your
machine and read the table it prints. There are no recorded rps or latency
numbers here on purpose.

## Prerequisites

- Rust 1.93.0+, target `wasm32-wasip2` (`rustup target add wasm32-wasip2`)
- Wasmtime CLI (46.0.1 is the repo's blocking reference:
  `cargo install wasmtime-cli --version 46.0.1 --locked`)
- Python 3 (stdlib only; used by `scripts/compare.py`)
- `curl` (readiness check)

Optional: [`hey`](https://github.com/rakyll/hey) or `wrk` if you want an
independent load generator. The bundled script does not require them.

## Endpoints

Both backends serve the same routes from `app/`:

| Method | Path | What it measures |
|---|---|---|
| `GET` | `/` | Simple SSR HTML page |
| `GET` | `/api/get_test` | GET server function (soak-style JSON body `"GET response"`) |
| `POST` | `/api/post_test` | JSON server function; body `{"msg":"hello"}` |
| `POST` | `/api/form_test` | Form server function; body `msg=hello` |
| `GET` | `/static/hello.txt` | Static file (`public/hello.txt`) |

Default listen addresses:

- WASI: `http://127.0.0.1:3000`
- axum: `http://127.0.0.1:3001`

## Build and run each side

From this directory:

```bash
make build          # both backends, --release
make wasi           # Wasmtime Preview 2 on :3000
make axum           # native axum on :3001
```

WASI is started with `--max-instance-reuse-count 128`. Preview 2 defaults to
rebuilding the component on every request; 128 is the production-oriented
setting documented in the crate's `PRODUCTION.md` and used by the Preview 2
soak. Override with `REUSE_COUNT=1 make wasi` if you want the host default.

Both sides use Cargo's default `--release` profile (not the wasm size
profile from `examples/counter`). That keeps the comparison on the
runtime/host rather than `opt-level = 'z'` versus native `opt-level = 3`.

## Compare

Same machine, both processes warmed, same concurrency and duration:

```bash
make compare
```

That builds both binaries, starts them, warms each endpoint (25 requests),
then runs a 10-second, 20-concurrency closed loop per endpoint per backend
and prints rps / p50 / p99 / errors plus `axum / wasi` ratios.

```bash
DURATION=30 CONCURRENCY=20 make compare
```

If the servers are already running:

```bash
SKIP_START=1 SKIP_BUILD=1 ./scripts/compare.sh
```

`hey` equivalents, after both servers are up (repeat for `:3001`):

```bash
hey -z 10s -c 20 http://127.0.0.1:3000/
hey -z 10s -c 20 http://127.0.0.1:3000/api/get_test
hey -z 10s -c 20 -m POST -T application/json -d '{"msg":"hello"}' \
  http://127.0.0.1:3000/api/post_test
hey -z 10s -c 20 -m POST -T application/x-www-form-urlencoded -d 'msg=hello' \
  http://127.0.0.1:3000/api/form_test
hey -z 10s -c 20 http://127.0.0.1:3000/static/hello.txt
```

A fair run:

1. One machine, nothing else pegging the CPUs
2. `--release` on both sides
3. Warm both before measuring (the script does this)
4. Identical duration, concurrency, and endpoint set
5. WASI Preview 2 with reuse 128 unless you are explicitly measuring cold instances
6. Do not compare a long soak on one side with a 10-second canary on the other

The Python probe is a closed loop (N workers, each waiting for a response
before sending the next), matching `scripts/load_runtime.py`. If achieved
concurrency sits well below the requested value, the client was the
bottleneck and the rps figure is not a server measurement.

## Layout

```
app/       shared Leptos routes and server functions
wasi/      Preview 2 component (`RouteTable` + `generate_routes_from`)
axum/      native `leptos_axum` + tokio
public/    static file served at `/static/hello.txt`
scripts/   compare.sh starts both; compare.py prints the table
```

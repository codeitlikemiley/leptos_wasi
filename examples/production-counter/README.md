# Production counter persistence service

The `examples/counter` application uses this service when started with
`make wasmtime` or `make spin`. It provides one durable state boundary for both
runtimes while keeping database ownership outside the WASI guest.

```text
trusted ingress -> Leptos WASI terminal -> private counter-store -> SQLite
```

The native store implements atomic increments and idempotency. It is a private
service, not a public proxy or a publishable crate. Production deployments must
place it on private networking and authenticate the terminal with mTLS or a
service mesh; no database or signing secret belongs in the guest component.

Start the store directly when you want to exercise its API without the Leptos
example:

```bash
mkdir -p ../../data
DATABASE_URL=sqlite://$(pwd)/../../data/counter.sqlite3 \
  cargo run --release --manifest-path store/Cargo.toml
```

Exercise idempotency:

```bash
curl --fail --json '{"operation_id":"demo-operation-1"}' \
  http://127.0.0.1:4040/v1/counters/demo/increment
curl --fail --json '{"operation_id":"demo-operation-1"}' \
  http://127.0.0.1:4040/v1/counters/demo/increment
```

Both calls return the same value. A different operation ID increments once.
The Make targets configure this loopback service explicitly; builds and other
fixtures without `COUNTER_STORE_URL` retain the isolated session-counter
behavior. Production deployments should replace the local process boundary
with a private regional service and apply their own backup, replication, and
availability policy.

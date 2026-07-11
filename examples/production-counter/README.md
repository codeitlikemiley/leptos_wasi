# Production counter persistence service

The polished `examples/counter` application is intentionally session scoped so
it stays focused on SSR, islands, split WASM, and server functions. This
directory provides the durable state boundary for a production counter.

```text
trusted ingress -> Leptos WASI terminal -> private counter-store -> PostgreSQL
```

The native store implements atomic increments and idempotency. It is a private
service, not a public proxy or a publishable crate. Production deployments must
place it on private networking and authenticate the terminal with mTLS or a
service mesh; no database or signing secret belongs in the guest component.

Start PostgreSQL and the store:

```bash
docker compose up -d postgres
DATABASE_URL=postgres://counter:counter@127.0.0.1:55432/counter \
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
The Leptos example does not silently enable this service: persistence changes
the trust and deployment model and must be selected explicitly by an
application. The store API is the reference boundary for that integration.

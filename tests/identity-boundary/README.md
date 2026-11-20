# Identity boundary

Host test that the two trusted-context designs stay distinct:

- `wasi-http-metadata::AuthContextV1` / `VerifiedAuthContext` — unsigned portable middleware envelope
- `wasi_auth::TrustedContextCodec` / `VerifiedRequestContext` — HMAC-SHA-256 envelope bound to one request

Both use the header name `x-wasi-auth-context`. This package checks that each codec rejects the other's wire shape. It is not part of the `leptos_wasi` library and it does not copy either implementation; it depends on the sibling `wasi-auth` checkout pinned in `tests/middleware/components.lock.toml`.

```bash
cargo test --manifest-path tests/identity-boundary/Cargo.toml --locked
```

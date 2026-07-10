# Authorization lifecycle fixture

This fixture runs the exact local final-WASIp3 middleware artifacts around the
existing `tests/test-app` Leptos service:

```text
request-id -> authn-policy -> authz-http -> downstream-probe -> Leptos
```

The deterministic AuthZEN PDP selects allow, provider failure, malformed
response, first-byte timeout, body timeout, saturation, and disconnect cases
from a canonical `x-request-id`. It never logs headers or request bodies. The
runner rejects dirty or revision-mismatched companion repositories by default,
verifies artifact checksums and final WIT contracts, and scans all runtime logs
for credential, cookie, query, and identity sentinels.

The Leptos application supplies the delayed and failing streams. Since
`leptos_wasi` intentionally does not expose an HTTP trailer API, the transparent
probe supplies one dedicated trailer response. That assertion proves the
authentication and authorization components preserve an opaque downstream
trailer; it does **not** claim that `leptos_wasi` can originate trailers.

Run the gate from the repository root:

```bash
rtk bash scripts/run-authz-lifecycle-e2e.sh
```

`AUTHZ_LIFECYCLE_ALLOW_DIRTY_COMPANIONS=1` exists only for local development.
Results produced with that override are not release evidence.

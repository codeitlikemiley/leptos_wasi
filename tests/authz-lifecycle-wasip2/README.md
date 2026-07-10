# WASIp2 authorization lifecycle fixture

This real-host fixture injects `leptos_wasi::wasip2::WaitPoll` into the
framework-neutral `wasi-authz-client` WASIp2 transport. It uses the authenticated
native Cedar PDP for allow and recovery decisions, and the final-WASIp3 fault
PDP's drip-body response to prove one absolute timeout across response frames.

The runner performs ten timeout/cancellation cycles. Every response publishes
the internal release-probe queue depth, and each cycle plus the final Cedar
recovery must report zero queued pollables. Runtime logs are scanned for PDP
credentials, cookie sentinels, query sentinels, and identity data.

Run from the repository root:

```bash
rtk bash scripts/run-authz-wasip2-lifecycle-e2e.sh
```

The companion authorization checkout and native PDP checksum are strict by
default. `AUTHZ_WASIP2_LIFECYCLE_ALLOW_DIRTY_COMPANION=1` is development-only
and cannot produce release evidence.

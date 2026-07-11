# Performance baseline

This file records the local comparison used while hardening 0.4.0. It is a
regression baseline, not a universal service-level objective: host hardware,
component pooling, ingress, and the application route tree materially affect
the absolute numbers.

## Historical 0.3.2 versus 0.4.0 candidate

Measured on 2026-07-10 on Apple Silicon with Wasmtime 45.0.0, Spin 4.0.0,
20 concurrent clients, a warm server-function route, and a 30-second paired
run. Both versions were built from the immutable `v0.3.2` worktree and the
current candidate using the same resolved dependencies and probe.

| Host | Preview | requests/s 0.3.2 to 0.4 | first-byte p99 ms 0.3.2 to 0.4 | total p99 ms 0.3.2 to 0.4 | p99 change | failures |
|---|---|---:|---:|---:|---:|---:|
| Wasmtime | P2 | 4840.82 to 4947.61 | 11.873 to 11.172 | 12.707 to 11.914 | -6.24% | 0 to 0 |
| Spin | P2 | 5098.68 to 5099.91 | 9.497 to 9.348 | 10.084 to 9.731 | -3.50% | 0 to 0 |
| Wasmtime | P3 | 5285.73 to 5321.33 | 10.004 to 9.952 | 10.696 to 10.658 | -0.36% | 0 to 0 |
| Spin | P3 | 5368.80 to 5399.59 | 9.264 to 8.524 | 9.702 to 8.899 | -8.28% | 0 to 0 |

The candidate stayed inside the accepted five-percent p99 regression budget in
all four comparisons. A first implementation regenerated Leptos routes on
every server-function request and failed this gate; 0.4 now caches the
type-stable generated route list while continuing to validate exclusions,
duplicate expansions, unsupported static SSR, and registration errors.

RSS was sampled once per second. Candidate start to end values were
102,416 to 34,208 KiB (Wasmtime P2), 17,120 to 17,120 KiB (Spin P2),
107,744 to 35,776 KiB (Wasmtime P3), and 17,216 to 17,216 KiB (Spin P3).
No candidate showed monotonic growth during
these probes. Absolute Wasmtime RSS varied substantially when two JIT
processes were measured concurrently, so plateau, not cross-process absolute
RSS, is the release criterion.

## Final WASI HTTP 0.3 baseline

Final-WASI measurements use Wasmtime 46.0.1 and the exact `wasip3` 0.7.0
bindings. A ten-second, 20-concurrency quick canary against the ordinary
`/api/get_test` route produced:

| Host | requests | requests/s | first-byte p99 ms | total p99 ms | failures | RSS start/end KiB | final-quarter growth KiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| Wasmtime 46.0.1 | 54,136 | 5,379.892 | 8.597 | 9.009 | 0 | 109,456 / 50,240 | -4,640 |

This quick probe is not the ten-minute release soak. It establishes that the
final component serves successfully and provides a concrete point for the
longer paired evidence.

Spin 4.0.0 produced no final-WASI measurement: it rejects the component because
the `wasi:http/types@0.3.0` resource implementation is missing. Its native
middleware commit also hard-codes the March RC handler world. Both Spin paths
are expected-failure canaries; only Wasmtime 46 is a blocking final-WASI
runtime. Release evidence must include generated comparison JSON rather than
reusing the historical RC-era table above.

## Reproducing release evidence

Pull-request CI runs paired ten-minute, 100-concurrency baseline and candidate
probes for Wasmtime P2/P3 and Spin P2. It uploads baseline, candidate, and
comparison JSON in one artifact per combination. Run the same probe locally
with, for example:

```bash
HOST=wasmtime PREVIEW=p3 DURATION=600 CONCURRENCY=100 \
  ./scripts/soak-test-app.sh
```

The JSON includes status counts, failures, throughput, first-byte and total
latency percentiles, and a timestamped host RSS timeline. A release requires zero unexpected
responses, p99 no more than five percent above its retained comparison
baseline, and a stable final-quarter memory plateau.

## Final-WASI middleware promotion gate

The component-middleware alpha has a separate realistic gate driven by
`scripts/benchmark-middleware-leptos.sh`. It alternates five wrapped/unwrapped
pairs, warms each profile, then records 30 seconds at concurrency 100 against a
50/50 mix of streaming SSR and a server function. The fixed promotion budget is
10% for first-byte p99, total p99, and throughput.

On 2026-07-10, the pure pass-through control passed: throughput changed -1.62%,
first-byte p99 -5.65%, and total p99 -8.85%. This rules out the final-WASI
component boundary alone. The optimized fused `secure-defaults` profile still
failed with zero request errors:

| Metric | Unwrapped median | Fused median | Change | Budget |
|---|---:|---:|---:|---:|
| First-byte p99 | 7.961 ms | 12.527 ms | +57.36% | <= +10% |
| Total p99 | 8.934 ms | 13.578 ms | +51.98% | <= +10% |
| Throughput | 32,131.58 requests/s | 22,787.77 requests/s | -29.08% | >= -10% |

The optimized component parses only policy-relevant headers and applies exact
edits to one cloned WASI header resource; it no longer performs a second
`copy-all`, a complete field-vector clone, or a quadratic full-header diff.
The remaining measured cost is the immutable request and response header
reconstruction plus transmission-result bridging required to preserve bodies,
trailers, cancellation, and post-commit errors. This is a stable-promotion
blocker, not a reason to relax the threshold. The 0.5 integration remains
explicitly alpha/experimental until the unchanged gate passes on a pinned
runtime.

## Full authentication and authorization chain

The promotion topology now removes the extra Wasmtime AuthZEN PDP hop:
relationship and hybrid operations use the typed `SpiceDbProvider` directly
from a private terminal, while Cedar remains embedded. A new Rust/Hyper load
driver replaces `urllib` for this gate and reports per-process CPU/RSS,
successful and failed latency histograms, first-byte latency, status classes,
and hangs.

A 2026-07-11 two-terminal, 500-request concurrency-100 diagnostic completed
with zero failures. The direct hybrid path measured 54.975 ms first-byte and
total p99, so it still fails the unchanged 25 ms gate. This short run proves
that bounded admission and replica tie-breaking removed the observed 503
saturation; it is not release evidence and does not replace five 5,000-request
repetitions or the ten-minute soak. Run lifecycle selection and the 1/2/4
replica matrix before promotion.

`scripts/benchmark-authz-full-chain.sh` uses the real composed chain, a local
authentication broker, the final-WASI Cedar PEP, and the final-WASI SpiceDB
PDP. It warms 500 requests and then sends 5,000 form-encoded server-function
requests at concurrency 100. Its fixed target is zero unexpected responses and
both first-byte and total p99 at or below 25 ms.

On 2026-07-10, the corrected isolated-port runner failed that gate on the
pinned Wasmtime 46.0.1 path: 2,317 of 5,000 requests returned controlled 503,
first-byte p99 was 192.975 ms, and total p99 was 275.921 ms. A 1,000-request
diagnostic with the highest still-bounded authentication admission setting
returned zero failures but still measured 97.014 ms first-byte p99 and 97.434
ms total p99. This distinguishes the explicit admission limit from the
remaining sustained final-WASI transport/PDP pressure; it does not establish a
safe capacity setting. The full-chain benchmark and ten-minute soak remain
failing alpha gates until the underlying saturation and latency behavior is
removed and the unchanged command passes.

The corresponding ten-minute, 100-concurrency soak completed on the same
isolated Wasmtime 46.0.1 path, but also failed as a release gate: it completed
69,759 requests at 116.229 requests/s and recorded 571,879 failed attempts,
with first-byte p99 7,153.217 ms and completed-response p99 7,341.842 ms. RSS
rose from 51,600 KiB to a 95,168 KiB high-water mark, then finished at 49,664
KiB (a -6,544 KiB final-quarter change across 596 samples). That non-monotonic
RSS result is useful evidence against an obvious unbounded-memory leak; it does
not offset the severe latency and request-failure regression. The retained
result is `target/authz-full-chain-soak/result.json`; it is diagnostic alpha
evidence, not a passing production-soak claim.

With the current exact artifacts and authentication admission default at 128,
a fresh 1,000-request, 100-concurrency domain-only run returned zero failures
but measured 73.841 ms first-byte p99 and 74.258 ms total p99. Raising
Wasmtime's instance reuse limits to 512 total and 128 concurrent improved the
same run to 67.650 ms and 68.921 ms respectively, still above the fixed 25 ms
target. The earlier controlled-503 saturation is therefore no longer the
dominant symptom in this configuration; sustained final-WASI request/PDP
latency under active concurrency remains the blocker.

The capacity-localization harness is configurable through
`scripts/benchmark-authz-capacity-matrix.sh`. It compares the default typed
authorization path with the optional coarse HTTP PEP across offered-rate
steps. The rate-limited diagnostic is an offered-load measurement, not a
replacement for the 100-active-request release gate. Set
`MIDDLEWARE_DIAGNOSTICS=1` to correlate controlled failures with fixed stage
labels in the broker, PDP, and terminal logs.

After the exact middleware (`27660a3`) and authorization (`f788740d`) revisions
were pinned, a 100-request Wasmtime canary at 20 active clients passed both
profiles with no failures. The default typed domain profile measured 11.367 ms
first-byte p99, 11.372 ms total p99, and 2,378 requests/s; the optional coarse
HTTP-PEP profile measured 16.284 ms first-byte p99, 16.289 ms total p99, and
1,598 requests/s. These are correctness and low-load localization canaries,
not replacements for the failing 100-active-request and ten-minute gates.

The short five-pair diagnostic for the fused `secure-defaults` component also
continues to exceed the promotion budget (28.23% first-byte p99, 35.54% total
p99, and -28.82% throughput at 20 clients). Reusing the response header handle
removed one redundant host lookup and improved all three measurements, but the
remaining cost is not an artifact-pin or high-concurrency-only failure; it is
the immutable WASIp3 request/response header reconstruction boundary documented
above.

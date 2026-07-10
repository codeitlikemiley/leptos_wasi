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

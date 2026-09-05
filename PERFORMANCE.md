# Performance baseline

This file records the local comparison used while hardening 0.4.0. It is a
regression baseline, not a universal service-level objective: host hardware,
component pooling, ingress, and the application route tree materially affect
the absolute numbers.

## Supported soak budgets

CI (`.github/workflows/main.yaml`) is the gate that ships:

| Lane | max latency regression | max throughput regression |
|---|---:|---:|
| Wasmtime Preview 2 | 12% | 10% |
| Spin Preview 2 | 12% | 10% |
| Wasmtime Preview 3 | 8% | 8% |

The 0.3.2-versus-0.4.0 table that follows is historical. It used a five-percent
p99 budget that CI no longer enforces.

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
every server-function request and failed this gate.

The fix recorded here was a type-keyed cache of the generated route list. That
description is no longer accurate, and the correction is instructive: the cache
was a `thread_local!`, and both supported hosts instantiate a fresh component
per request, so it started empty on every lookup and never returned a hit in
production. It has been removed. Route discovery is now skipped outright on
requests that cannot use the SSR router, which is what 0.3.2 did and what this
paragraph originally set out to achieve. See
[Where the 0.4 overhead comes from](#where-the-04-overhead-comes-from).

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

The original Spin 4.0.0 run produced no final-WASI measurement: it rejects the component because
the `wasi:http/types@0.3.0` resource implementation is missing. Its native
middleware commit also hard-codes the March RC handler world. Both Spin paths
are expected-failure canaries. Tagged Spin 4.0.2 has the same ABI limitation.
Pinned Spin main now runs plain final-WASI terminals, but composed middleware
is blocked by its default CPU-metrics panic; only Wasmtime 46 is a blocking final-WASI
runtime. Release evidence must include generated comparison JSON rather than
reusing the historical RC-era table above.

## Measured 0.3.2 versus 0.4 soak deltas

These are the first numbers produced after the soak gates were repaired; every
earlier run reported success unconditionally because the checker was piped into
`tee`, which discarded its exit status.

| Run | Lane | first-byte p99 | total p99 | throughput |
|---|---|---:|---:|---:|
| 1 | Wasmtime P2 | +8.59% | +8.55% | -6.62% |
| 2 | Wasmtime P2 | — | — | -7.11% |

The two throughput samples differ by half a percentage point, and both sides
pin identical third-party dependency versions in `tests/test-app/Cargo.lock`,
so the delta is not upstream drift. It has since been located - see
[Where the 0.4 overhead comes from](#where-the-04-overhead-comes-from) - and
the paragraph below records the measurement limit that made locating it hard.

### What a single paired run can and cannot show

An attempt to reproduce the delta locally, on a different machine, ran five
alternating 60-second pairs of the same two guests at concurrency 100:

| Pair | 0.3.2 rps | 0.4 rps | delta |
|---:|---:|---:|---:|
| 1 | 1464.7 | 1546.3 | +5.57% |
| 2 | 1620.1 | 1536.8 | -5.14% |
| 3 | 1573.6 | 1417.1 | -9.95% |
| 4 | 1519.3 | 1507.2 | -0.79% |
| 5 | 1436.8 | 1474.1 | +2.60% |

The per-pair delta changes sign, and the mean is -1.75%. More telling, the
*same* binary measured across those runs spans 12.8% (baseline) and 9.1%
(candidate), so on that machine the noise is larger than the effect and the
comparison cannot resolve it.

That does not refute the CI figure. Noise falls with the square root of the
sample window, so a 12% spread over 60 seconds corresponds to roughly 4% over
CI's 600 seconds, which is below the ~7% CI reports. The CI number is
plausibly real.

It does establish a limit on the method. The soak runs the baseline once and
the candidate once, in that fixed order, with no replication and no dispersion
estimate. A design like that cannot separate a code regression from anything
that drifts monotonically across the job - thermal behaviour, a co-tenant
ramping up - because whatever runs second is always the candidate. Before the
delta is attributed to a specific change, the comparison needs either
alternated ordering or repeated pairs compared by median. Both cost CI time on
a lane already close to its 35-minute timeout, so that is a deliberate
trade-off rather than an oversight to fix silently.

The absolute Wasmtime P3 lane measured 87.66 ms total p99 and -168 to +88 KiB
final-quarter RSS growth across runs. Its budget is set from that observation
and should tighten once several more runs establish the spread.

## Where the 0.4 overhead comes from

### The method that resolved it

Pooled statistics could not separate the effect from machine drift: across four
alternating 60-second pairs the box slowed 28% end to end, and 0.3.2 measured
anywhere from 959 to 692 rps. Pooling that into a standard deviation reports
"-17.07%, sd 14.91%" and looks like noise.

Comparing **adjacent pairs** resolves it. Both members of a pair run seconds
apart under the same conditions, so drift cancels:

| Pair | 0.3.2 rps | 0.4 rps | ratio |
|---:|---:|---:|---:|
| 1 | 959.6 | 822.0 | -14.3% |
| 2 | 911.6 | 703.2 | -22.9% |
| 3 | 712.4 | 594.3 | -16.6% |
| 4 | 692.1 | 597.3 | -13.7% |

Four of four in the same direction, mean -16.9%, sd 4.1pp, standard error
2.0pp - eight standard errors from zero. Any future comparison on a noisy host
should use paired ratios rather than pooled means; the earlier section's
conclusion that the method "cannot resolve it" applies to unpaired sampling
only.

### Which commit

A paired bisect at concurrency 1 against 0.3.2, five reps per candidate,
measured `/api/get_test`:

| Commit | vs 0.3.2 | stderr |
|---|---:|---:|
| `6da6ed7` route discovery | -13.71% | 1.91pp |
| `663e1a9` final WASI 0.3 | -14.63% | 1.89pp |
| `2141837` 0.4 candidate | -9.94% | 0.84pp |
| `dd35de9` 0.4.2-rc.1 | -14.01% | 2.85pp |
| `e798edb` cache identity | -10.20% | 2.35pp |
| `8578fd2` nosniff | -12.81% | 1.12pp |

There is no step anywhere in the range - the cost is already present at the
earliest commit that builds and is flat across six commits after it. That
places the origin at `d1f9308`, the additive-adapter rewrite, which is the
parent of the first measurable point and the only commit in the window that
rewrites the request path (3480 changed lines). `d1f9308` itself cannot be
measured in isolation: its test application predates
`leptos_wasi::prelude::Handler` and does not compile.

### Where the time goes

Both supported hosts instantiate a **fresh component per request**. This is
observable rather than inferred: a counter accumulated in a guest static reads
zero after 18,000 requests, and Wasmtime's in-process guest profiler collects
no samples because no store survives long enough to be sampled.

Guest-side stage timing on `/api/get_test` (n=14,125, 1054 us mean):

| Stage | Time | Share |
|---|---:|---:|
| registration - 13x `with_server_fn` + `generate_routes` | 183 us | 17% |
| handle - render and send | 92 us | 9% |
| everything else - instantiation, host, HTTP, client | 779 us | 74% |

Three quarters of a request is spent before any handler code runs. That is why
no single code change recovers the delta: the whole guest handler is a quarter
of the budget, and the regression is spread across it rather than sitting on
one line.

Do not read that last row as "instantiation costs 779 us". Measuring instance
reuse (below) puts instantiation at roughly **107 us** per request; the rest of
that row is the host's HTTP path, the socket, and the probe's own client
overhead. An earlier revision of this document leaned on the larger figure to
argue that module size drove the regression through instantiation work. That
argument was wrong twice over - the arithmetic never supported it, and
instantiation is a seventh of the bucket it was attributed to.

The cost splits by response class. A 404, which short-circuits before route
matching, pays about 0.07 ms; a 200 pays about 0.14 ms. Both 200 endpoints
regress identically (-13.83% for a server function, -13.64% for a zero-byte
static file) despite having entirely different producers, so the extra tier is
shared success-path work, not anything specific to either.

### What was excluded, and why size is not the cause

Each of these was tested by building a variant and measuring it paired against
0.3.2, not by reading the diff:

| Hypothesis | Result |
|---|---|
| Post-flush `WaitPoll` on the p2 write path | +0.8pp, median -0.8pp |
| 0.3.2's `blocking_write_and_flush` write loop restored | +2.1pp, median +0.5pp |
| Removing the dead route cache | +2.11pp, se 1.61pp |
| Tracing overhead | Compiles to a unit struct and empty functions |
| `parts.clone()` per request | Identical in 0.3.2 |

A zero-byte static file regresses as much as a body-producing endpoint, which
rules out byte writing independently of the two write-path experiments.

Module size correlates with the regression across commits - it steps 8.5% at
the same commit and stays flat - but it is not the mechanism. Only the `data`
section is copied per instantiation, and it grew 15,836 bytes, on the order of
a microsecond of memcpy rather than the tens of microseconds observed. The
growth is 8.6% in `code`, which is compiled once at startup, and 9.4% in
name and debug sections, which cost nothing at runtime. Stripping the binary
would remove roughly 40% of the file and change latency by zero. Both test
applications already build with `opt-level='z'`, fat LTO, `codegen-units=1`
and `panic="abort"`, so there is no profile slack to recover either.

### What the WASIp3 lane found on its first run

Giving the p3 lane a baseline immediately produced a failure: -6.47%
first-byte p99, -6.40% total p99, -5.63% throughput against `663e1a9`,
against a 5% budget.

A paired re-measurement of the same two commits puts the real figure at
**-2.51%, sd 1.74pp, standard error 0.71pp** over six pairs - five of six
negative, so a genuine regression, but less than half what the lane reported.
The gap is the drift this document warns about two sections up: the lane runs
the baseline once and the candidate once, in that fixed order, so the candidate
absorbs whatever the runner does over twenty minutes.

This is worth recording as a worked example. The lane was not wrong to fail -
something did regress - but the number it produced would have led to chasing a
6% effect that is actually 2.5%, which is how this investigation started in the
first place. The budget is set at 8% to cover the measured effect plus that
drift, and the honest way to tighten it is to alternate the runs rather than to
lower the number.

### What it costs in practice

The benchmark endpoint returns the 12-byte string `"GET response"` with no
rendering, database, or middleware in the loop, so it measures this crate's
per-request plumbing with application work set to zero. The regression is
roughly 0.15 ms added to a ~1 ms floor. On any page that renders real content
that is a small share of the response, and the deployment target in this
document is unaffected. It is documented because a fixed per-request cost is
worth knowing about, not because it is user-visible.

### The registration stage, resolved

The 183 us registration stage turned out not to need moving to build time. It
needed not to run.

0.3.2 returned early from route generation whenever the request was already
claimed, under the comment "if we matched a server function, we do not need to
go through all of that". `d1f9308` removed that early return: 0.4 ran full
discovery and then dropped every entry through a `if shortcut { continue; }`
in the registration loop.

Restoring the skip, measured paired against 0.3.2 over five reps:

| Endpoint | 0.4 | with the skip | recovered |
|---|---:|---:|---:|
| `/api/get_test` (claimed) | -13.13% | -5.78% | +7.35pp |
| `/definitely-not-a-route` (uses the router) | -3.71% | -2.45% | +1.26pp |

The asymmetry is the evidence. A 404 has to consult the router to know it is a
404, so it discovers routes in both versions and barely moves - within its own
error bar. A uniform improvement across both would have meant the change was
doing something other than what it claims.

This is also what makes the two-tier structure above fall out: claimed requests
regress by roughly the discovery cost, and unclaimed ones show only the
residual. About 5.8% remains on claimed requests and is still unexplained.

Build-time route generation is therefore only worth considering for genuine SSR
requests, which do need the router on every fresh instance. That is a narrower
prize than it appeared before the skip was restored.

### Instance reuse: the largest lever is a host flag

`wasmtime serve` bounds how many requests one component instance may serve with
`--max-instance-reuse-count`. It **defaults to 1 for WASIp2 and 128 for
WASIp3**. A Preview 2 deployment therefore builds a fresh component for every
request unless it says otherwise, and that default - not this crate - is the
largest single cost in a Preview 2 request.

Measured on the merged tree, five paired reps at concurrency 1 against the
default configuration:

| Configuration | rps | mean | vs default |
|---|---:|---:|---:|
| default | 1161.4 | 853 us | - |
| `--max-instance-reuse-count 128` | 1326.1 | 746 us | **+14.44%** (se 3.45pp) |
| `-O pooling-allocator=y` | 1153.5 | 858 us | +1.39% (se 2.18pp) |
| both | 1346.0 | 735 us | +13.65% (se 1.42pp) |

Instance reuse is worth more than the entire 0.4-versus-0.3.2 regression
documented above. The pooling allocator is indistinguishable from noise on its
own and adds nothing on top of reuse, so it is not part of the recommendation.

The saving is about 107 us per request, which is what instantiation actually
costs here - not the 779 us of the "everything else" row above.

The e2e suite is the check that matters, because reuse makes guest statics -
the executor cell, the pollable queue - outlive a request. Set
`LEPTOS_WASI_MAX_INSTANCE_REUSE` to run it under reuse:

```
LEPTOS_WASI_MAX_INSTANCE_REUSE=128 \
  cargo test --locked --test e2e test_e2e_wasip -- --ignored --test-threads=1
```

Both Preview 2 and Preview 3 pass, and the suite finishes in 1.90 s against
5.10 s at the default - the same speedup measured a second way. That is
evidence for the paths the suite covers, which include server functions, static
assets, SSR, islands, redirects, and a panicking server function. It is not a
general proof that every application is safe to reuse: an application holding
its own request-scoped state in a static would be, and this crate cannot check
that for it.

### Reusing the route table across those 128 requests

The 14.44% above is instantiation reuse: the host keeps the component. It does
not skip route discovery. `generate_routes` still renders the application on
every SSR request, so the 183 us registration stage is paid again even when
the instance is reused.

`RouteTable::discover` is that discovery, once per instance.
`generate_routes_from` installs the table with an `Rc` clone. Host-native
divan (`cargo make bench`, `benches/route_discovery.rs`) on this machine,
medians:

| Step | 3 routes | 8 routes | 32 routes |
|---|---:|---:|---:|
| `validate_route_table` | 7.0 us | 11.8 us | 35.6 us |
| `RouteTable::discover` | 7.3 us | 12.5 us | 37.4 us |
| `RouteTable` clone (`generate_routes_from`) | 41 ns | 41 ns | 41 ns |

That synthetic app is not the test-app: in-guest discovery plus registration
on `/api/get_test` remains **183 us**. `RouteTable::discover` matches
`validate_route_table` (the extra `routefinder` insert is in the noise). The
clone sits on the 41 ns timer floor - the per-request install cost under
reuse. Under `--max-instance-reuse-count 128`, an SSR-only workload
amortizes the 183 us across up to 128 requests (~1.4 us/request). Claimed
paths already skip discovery without a table.

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

The Wasmtime results below are reference-runtime diagnostics. They remain
correctness and regression evidence but do not block stable library packaging
on the 25 ms deployment SLO. Spin is the preferred production-performance
target because it pools outbound HTTP and supports concurrent Preview 3
instance reuse. Promotion remains blocked until a tagged Spin release supports
final `wasi:http@0.3.0`; this project will not downgrade to an RC ABI.

The promotion topology now removes the extra Wasmtime AuthZEN PDP hop:
relationship and hybrid operations use the typed `SpiceDbProvider` directly
from a private terminal, while Cedar remains embedded. A new Rust/Hyper load
driver replaces `urllib` for this gate and reports per-process CPU/RSS,
successful and failed latency histograms, first-byte latency, status classes,
and hangs.

The 2026-07-12 consolidated-source matrix completed all five 5,000-request,
concurrency-100 repetitions per profile on Wasmtime 46.0.1. It was a dirty-tree
diagnostic run, so signed artifact-set verification was intentionally not
claimed. Every profile failed because at least one request had an unexpected
status or transport outcome; only two of five paired anonymous-edge samples
met both unchanged 10% regression limits.

| Profile | Median requests/s | Median p99 | Worst p99 | Failures across five runs |
|---|---:|---:|---:|---:|
| Proxy baseline | 19,684.38 | 13.431 ms | 16.687 ms | 108 |
| Anonymous edge | 18,390.11 | 15.431 ms | 47.647 ms | 114 |
| Authentication only | 14,892.11 | 21.151 ms | 24.255 ms | 354 |
| Embedded Cedar | 10,590.42 | 25.471 ms | 45.407 ms | 630 |
| Direct relationship | 6,813.00 | 38.015 ms | 299.775 ms | 6,534 |
| Direct hybrid | 5,893.07 | 54.495 ms | 102.911 ms | 8,265 |
| Cedar-first denial | 10,503.41 | 21.407 ms | 152.447 ms | 1,260 |

The same audit found and fixed two defects in the evidence harness. Terminal
health could change between two selection passes and leave an empty candidate
set, causing a modulo-by-zero worker panic. Terminal selection now uses one
health/load snapshot and returns a controlled unavailable response if none are
healthy. The closed-loop driver also started its 30-second drain deadline
immediately after spawning workers, truncating every requested ten-minute run
at 30 seconds. Its drain deadline now begins after the configured load
deadline. Regression tests cover both cases, and failed load runs retain the
structurally validated report plus ingress diagnostics.

The ingress diagnostics now distinguish an unfinished downstream response body
(`response_body_aborts`) from an observed request cancellation and an observed
client disconnect. Dropping `GuardedBody` alone is not sufficient evidence of
a client disconnect, so it no longer increments all three counters. Historical
soak values that reported identical cancellation and disconnect counts must not
be used as client-behavior evidence.

The load driver also supports scheduled open-loop arrivals. Open-loop latency
is measured from the intended arrival time, including client scheduling delay
and saturation, so overload is not hidden by coordinated omission. Run
`scripts/benchmark-trusted-ingress-open-loop.sh` after the five-sample capacity
matrix; it exercises 50%, 70%, and 90% of each profile's measured median
capacity and preserves the unchanged 25 ms gate.

After those fixes, a two-terminal, 600.009-second, concurrency-100 soak
completed 2,586,955 responses (4,311.53 responses/s) with zero canceled or
hung requests. It still failed promotion: 1,005,636 responses had unexpected
statuses, 42,793 attempts had transport failures, successful-response total
p99 was 81.407 ms, response-header p99 was 72.767 ms, and first-body-byte p99
was 73.023 ms. All five sampled processes passed the final-quarter RSS gate;
the largest positive final-quarter change was 992 KiB in the authentication
broker, while ingress changed by -192 KiB. Diagnostics localize the remaining
failure to sustained terminal/native transport pressure, not an admission
queue leak. `scripts/check-trusted-load-soak.py` now writes `summary.json` and
enforces duration, failures, all three 25 ms p99 limits, final process
liveness, and `min(32 MiB, 10%)` per-process final-quarter RSS growth.

The per-process memory allowance takes the *tighter* of the two bounds. Taking
the looser one meant a larger process bought a larger leak allowance, so a
small process was held to a ceiling it could never reach and a small leak went
unseen. Where no starting sample exists there is nothing to be proportional
to, so the absolute ceiling stands alone.

The 25 ms p99 figure is a per-request deployment target and is measured at low
concurrency; the single-digit p99 values recorded above come from those runs.
It is not reachable in a saturated closed-loop soak: at concurrency 100 the
mean latency is fixed by `concurrency / throughput`, which for the observed
2623 requests per second is 38 ms, and p99 necessarily exceeds the mean. The
concurrency-100 lane therefore carries its own load-appropriate budget rather
than the deployment target. Both numbers are real; they describe different
load profiles.

A 2026-07-11 two-terminal, 500-request concurrency-100 diagnostic completed
with zero failures. The direct hybrid path measured 54.975 ms first-byte and
total p99, so it still fails the unchanged 25 ms gate. This short run proves
that bounded admission and replica tie-breaking removed the observed 503
saturation; it is not release evidence and does not replace five 5,000-request
repetitions or the ten-minute soak. Run lifecycle selection and the 1/2/4
replica matrix before promotion.

The quick lifecycle harness selected reuse `512/16` from its deliberately
reduced smoke matrix. It measured 8.183 ms p99 at concurrency 16 and 38.623 ms
p99 at concurrency 100 with two terminals. A four-terminal ceiling probe did
not improve the result: it measured 49.535 ms p99 with zero admission queueing
or failures. Ingress diagnostics placed authentication below the target while
terminal first-byte occupied the 32--64 ms bucket. Isolated single-run profiles
measured 17.935 ms authn-only, 27.695 ms Cedar, 46.911 ms direct relationship,
47.359 ms hybrid allow, and 33.471 ms Cedar-first denial p99. These are
localization diagnostics, not promotion evidence; they identify the direct
relationship/terminal path as the next optimization target and prohibit adding
more than four replicas to mask it.

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

## Pinned Spin-main trusted-ingress measurement

The production topology can be measured with a plain Spin terminal while
keeping request ID, CORS, security headers, and authentication in native
trusted ingress:

```bash
./scripts/benchmark-trusted-ingress-spin-main.sh
DURATION=600 CONCURRENCY=100 ./scripts/soak-trusted-ingress-spin-main.sh
```

This avoids the guest-composition CPU-metrics panic and exercises Spin's final
outbound HTTP path to SpiceDB. Results remain experimental until the matching
runtime support appears in a tagged release. The existing guest-middleware
regression was measured on loopback without a broker or database call, so
regional database placement cannot explain that separate header-reconstruction
cost.

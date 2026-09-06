# Ten-minute soak history

CI (`.github/workflows/main.yaml`) runs a paired ten-minute, concurrency-100
soak on every pull request and every push to `main`. GitHub keeps those
artifacts for 14 days. This table is the repo-visible record after that, so a
later merge can be compared with earlier `main` soaks.

It is not a gate. The budgets in
[PERFORMANCE.md](./PERFORMANCE.md#supported-soak-budgets) still decide
pass/fail. Absolute rps and p99 move with the runner; the paired delta against
that job's baseline is the comparable signal. Newest rows go at the top. Do
not rewrite old rows.

`p99` here is total latency (`latency_ms` in the comparison JSON). First-byte
p99 stays in the artifact.

## After a merge to `main`

1. Wait for the merge commit's CI soak jobs (`Ten-minute soak / wasmtime / p2`,
   `p3`, and `spin / p2`).
2. Download the three `soak-*` artifacts from that run.
3. Print rows (quote reuse values that contain spaces):

   ```bash
   python3 scripts/format-soak-history.py \
     --date YYYY-MM-DD \
     --sha <merge-sha> \
     --pr <n> \
     --run <github-run-id> \
     --reuse wasmtime/p2=128 \
     --reuse 'wasmtime/p3=host default' \
     --reuse spin/p2=n/a \
     path/to/downloaded/artifacts
   ```

4. Paste the rows at the **top** of the table below.
5. Set `--reuse` from the soak job log (`instance reuse count: 128` or
   `host default`). Spin has no Wasmtime reuse flag. Do not invent numbers; if
   artifacts have expired, skip the row.

The same checklist applies when cutting a release, using that tag's CI run.

`--reuse` and `--baseline-ref` default to the current CI matrix (p2 baseline
`9689c68`, p3 baseline `663e1a9`). Override them if the lane's pin changes.

## Results

| Date | Commit | Lane | Reuse | Baseline | rps (base to cand) | p99 ms (base to cand) | Δ rps | Δ p99 | Gate | Notes |
|---|---|---|---|---|---:|---:|---:|---:|---|---|
| 2026-09-06 | 4713542 (#37) | Wasmtime P2 | 128 | 9689c68 | 2424.19 to 2729.14 | 96.29 to 83.74 | +12.58% | -13.04% | pass | CI run 34026698619; merge to main |
| 2026-09-06 | 4713542 (#37) | Wasmtime P3 | host default | 663e1a9 | 2533.74 to 2600.82 | 92.71 to 88.74 | +2.65% | -4.28% | pass | CI run 34026698619; merge to main |
| 2026-09-06 | 4713542 (#37) | Spin P2 | n/a | 9689c68 | 2413.45 to 2303.58 | 97.01 to 102.78 | -4.55% | +5.95% | pass | CI run 34026698619; merge to main |
| 2026-09-06 | 131f7b6 (#37) | Wasmtime P2 | 128 | 9689c68 | 2447.11 to 2757.24 | 95.29 to 83.18 | +12.67% | -12.70% | pass | CI run 34015218768; PR soak on head; merged as 4713542 |
| 2026-09-06 | 131f7b6 (#37) | Wasmtime P3 | host default | 663e1a9 | 2776.37 to 2779.09 | 81.38 to 81.43 | +0.10% | +0.06% | pass | CI run 34015218768; PR soak on head; merged as 4713542 |
| 2026-09-06 | 131f7b6 (#37) | Spin P2 | n/a | 9689c68 | 2496.80 to 2395.66 | 92.54 to 97.17 | -4.05% | +5.01% | pass | CI run 34015218768; PR soak on head; merged as 4713542 |
| 2026-09-05 | 5ad1759 (#36) | Wasmtime P2 | host default | 9689c68 | 2557.56 to 2533.70 | 90.10 to 90.56 | -0.93% | +0.51% | pass | CI run 33999470587; merge to main |
| 2026-09-05 | 5ad1759 (#36) | Wasmtime P3 | host default | 663e1a9 | 3947.97 to 3906.56 | 61.03 to 61.62 | -1.05% | +0.96% | pass | CI run 33999470587; merge to main |
| 2026-09-05 | 5ad1759 (#36) | Spin P2 | n/a | 9689c68 | 2533.46 to 2549.59 | 91.47 to 90.52 | +0.64% | -1.04% | pass | CI run 33999470587; merge to main |

## Seed sources

Numbers are from the uploaded comparison JSON, not from memory or rounded
summaries.

- **2026-09-06 / #37 merge to main.** Push to `main` after
  [#37](https://github.com/codeitlikemiley/leptos_wasi/pull/37),
  [run 34026698619](https://github.com/codeitlikemiley/leptos_wasi/actions/runs/34026698619)
  on
  [`4713542`](https://github.com/codeitlikemiley/leptos_wasi/commit/4713542bfdc4cb93637be7a762ace4f6bc2eb863).
  Wasmtime P2 candidate reuse 128 and Preview 3 host default are from that
  run's job logs.
- **2026-09-06 / #37.** Pull-request soak on head
  [`131f7b6`](https://github.com/codeitlikemiley/leptos_wasi/commit/131f7b681cada5353fb8f13c4a91509e84ea8378),
  [run 34015218768](https://github.com/codeitlikemiley/leptos_wasi/actions/runs/34015218768),
  merged to `main` as
  [`4713542`](https://github.com/codeitlikemiley/leptos_wasi/commit/4713542bfdc4cb93637be7a762ace4f6bc2eb863).
  Wasmtime P2 candidate reuse 128 is from that run's job log and
  `scripts/soak-test-app.sh` on the PR; Preview 3 omitted the flag (host
  default).
- **2026-09-05 / #36.** Push to `main` after
  [#36](https://github.com/codeitlikemiley/leptos_wasi/pull/36),
  [run 33999470587](https://github.com/codeitlikemiley/leptos_wasi/actions/runs/33999470587)
  on
  [`5ad1759`](https://github.com/codeitlikemiley/leptos_wasi/commit/5ad1759021915695b0d4bceb9bafceaa226e986f).
  `soak-test-app.sh` at that SHA did not pass `--max-instance-reuse-count`, so
  Wasmtime lanes are host default (Preview 2 default 1, Preview 3 default 128).

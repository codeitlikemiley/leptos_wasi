#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CAPACITY_SUMMARY="${CAPACITY_SUMMARY:-$ROOT/target/trusted-ingress-benchmark/summary.json}"
RESULTS="${RESULTS:-$ROOT/target/trusted-ingress-open-loop}"
REPETITIONS="${REPETITIONS:-5}"
DURATION="${DURATION:-60}"
TERMINAL_REPLICAS="${TERMINAL_REPLICAS:-2}"
HOST="${HOST:-wasmtime}"
mkdir -p "$RESULTS"

[[ -f "$CAPACITY_SUMMARY" ]] || {
  echo "capacity summary is required: $CAPACITY_SUMMARY" >&2
  exit 2
}

scenario_for_profile() {
  case "$1" in
    proxy-baseline) echo proxy-baseline ;;
    edge-anonymous) echo edge-anonymous ;;
    authn-only) echo authn-only ;;
    cedar) echo cedar ;;
    relationship-direct) echo relationship-direct ;;
    hybrid-direct) echo hybrid-direct ;;
    hybrid-deny) echo hybrid-deny ;;
    *) return 1 ;;
  esac
}

ingress_profile_for_profile() {
  case "$1" in
    proxy-baseline) echo proxy-baseline ;;
    edge-anonymous) echo edge-anonymous ;;
    *) echo edge-authenticated ;;
  esac
}

capacity_for_profile() {
  python3 - "$CAPACITY_SUMMARY" "$1" "$2" <<'PY'
import json, math, sys
summary, profile, fraction = sys.argv[1:]
data = json.load(open(summary, encoding="utf-8"))
capacity = float(data[profile]["requests_per_second_median"])
print(max(1, math.floor(capacity * float(fraction))))
PY
}

profiles=(proxy-baseline edge-anonymous authn-only cedar relationship-direct hybrid-direct hybrid-deny)
for profile in "${profiles[@]}"; do
  scenario="$(scenario_for_profile "$profile")"
  ingress_profile="$(ingress_profile_for_profile "$profile")"
  for percent in 50 70 90; do
    fraction="0.$percent"
    rate="$(capacity_for_profile "$profile" "$fraction")"
    for repetition in $(seq 1 "$REPETITIONS"); do
      output="$RESULTS/$profile/$percent/$repetition"
      mkdir -p "$output"
      status=0
      AUTHENTICATION_MODE=trusted_ingress AUTHZ=1 MIDDLEWARE=0 HOST="$HOST" \
        TRUSTED_INGRESS_HEALTH_CHECKS=0 \
        TERMINAL_REPLICAS="$TERMINAL_REPLICAS" \
        TRUSTED_INGRESS_PROFILE="$ingress_profile" \
        AUTHZ_FULL_CHAIN_BENCHMARK=1 AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 \
        AUTHZ_FULL_CHAIN_BENCHMARK_MODE=open-loop \
        AUTHZ_FULL_CHAIN_BENCHMARK_RATE="$rate" \
        AUTHZ_FULL_CHAIN_BENCHMARK_DURATION="$DURATION" \
        AUTHZ_FULL_CHAIN_SCENARIO="$ROOT/tests/trusted-ingress/scenarios/$scenario.toml" \
        AUTHZ_FULL_CHAIN_BENCHMARK_DIR="$output" \
        AUTHZ_FULL_CHAIN_BENCHMARK_SEED="$((20260712 + repetition))" \
        "$ROOT/scripts/run-trusted-ingress-browser.sh" || status=$?
      [[ -f "$output/result.json" ]] || exit "$status"
      python3 "$ROOT/scripts/validate-trusted-load-report.py" "$output/result.json"
      if [[ "$status" -ne 0 ]]; then
        echo "open-loop sample failed: profile=$profile rate=$rate repetition=$repetition" >&2
      fi
    done
  done
done

python3 - "$RESULTS" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
samples = []
passed = True
for path in sorted(root.glob("*/*/*/result.json")):
    report = json.loads(path.read_text())
    totals = report["totals"]
    failures = sum(totals[name] for name in (
        "status_failures", "transport_failures", "canceled", "hung"
    ))
    corrected = report["configuration"].get("scheduled_latency_correction") is True
    p99 = report["latency_ms"]["successful"]["p99"]
    sample_passed = corrected and failures == 0 and p99 <= 25
    passed &= sample_passed
    samples.append({
        "path": str(path.relative_to(root)),
        "corrected": corrected,
        "failures": failures,
        "p99_ms": p99,
        "passed": sample_passed,
    })
summary = {"samples": samples, "promotion_passed": passed and bool(samples)}
(root / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
if not summary["promotion_passed"]:
    raise SystemExit(1)
PY

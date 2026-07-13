#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESULTS="${RESULTS:-$ROOT/target/trusted-ingress-benchmark}"
REPETITIONS="${REPETITIONS:-5}"
TERMINAL_REPLICAS="${TERMINAL_REPLICAS:-1}"
BENCHMARK_SEED="${BENCHMARK_SEED:-20260711}"
HOST="${HOST:-wasmtime}"
mkdir -p "$RESULTS"

run_profile() {
  local profile="$1" scenario="$2" ingress_profile="$3" repetition="$4"
  local report="$RESULTS/$profile/$repetition/result.json"
  if ! AUTHENTICATION_MODE=trusted_ingress AUTHZ=1 MIDDLEWARE=0 HOST="$HOST" \
      TRUSTED_INGRESS_HEALTH_CHECKS=0 \
      TERMINAL_REPLICAS="$TERMINAL_REPLICAS" \
      TRUSTED_INGRESS_PROFILE="$ingress_profile" \
      AUTHZ_FULL_CHAIN_BENCHMARK=1 AUTHZ_FULL_CHAIN_BENCHMARK_ONLY=1 \
      AUTHZ_FULL_CHAIN_SCENARIO="$ROOT/tests/trusted-ingress/scenarios/$scenario.toml" \
      AUTHZ_FULL_CHAIN_BENCHMARK_DIR="$RESULTS/$profile/$repetition" \
      AUTHZ_FULL_CHAIN_BENCHMARK_SEED="$((BENCHMARK_SEED + repetition))" \
      "$ROOT/scripts/run-trusted-ingress-browser.sh"; then
    if [[ ! -f "$report" ]]; then
      echo "benchmark infrastructure failed before producing $report" >&2
      return 1
    fi
    python3 "$ROOT/scripts/validate-trusted-load-report.py" "$report"
    echo "warning: $profile repetition $repetition produced a valid failing performance sample" >&2
  fi
}

for repetition in $(seq 1 "$REPETITIONS"); do
  while IFS= read -r profile; do
    case "$profile" in
      edge-pair)
        run_profile proxy-baseline proxy-baseline proxy-baseline "$repetition"
        run_profile edge-anonymous edge-anonymous edge-anonymous "$repetition"
        ;;
      authn-only)
        run_profile authn-only authn-only edge-authenticated "$repetition"
        ;;
      cedar)
        run_profile cedar cedar edge-authenticated "$repetition"
        ;;
      relationship-direct)
        run_profile relationship-direct relationship-direct edge-authenticated "$repetition"
        ;;
      hybrid-direct)
        run_profile hybrid-direct hybrid-direct edge-authenticated "$repetition"
        ;;
      hybrid-deny)
        run_profile hybrid-deny hybrid-deny edge-authenticated "$repetition"
        ;;
    esac
  done < <(python3 -c '
import random, sys
profiles = ["edge-pair", "authn-only", "cedar", "relationship-direct", "hybrid-direct", "hybrid-deny"]
random.Random(int(sys.argv[1])).shuffle(profiles)
print("\n".join(profiles))
' "$((BENCHMARK_SEED + repetition))")
done

python3 "$ROOT/scripts/summarize-trusted-ingress-benchmark.py" "$RESULTS"

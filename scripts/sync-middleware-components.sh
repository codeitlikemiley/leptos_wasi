#!/usr/bin/env bash

set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/middleware-common.sh"

repository="$(middleware_repository)"
artifact_root="${repository}/artifacts"
destination="${MIDDLEWARE_ROOT}/tests/middleware-artifacts"
wasm_tools_bin="$(resolve_middleware_tool WASM_TOOLS_BIN wasm-tools "$(middleware_lock_value wasm_tools_version)")"
cosign_bin="$(resolve_middleware_tool COSIGN_BIN cosign "$(middleware_lock_value cosign_version)")"

[[ -f "${repository}/compatibility.toml" ]] || {
  echo "wasi-http-middleware checkout is unavailable: ${repository}" >&2
  exit 2
}

if [[ "${WASI_HTTP_MIDDLEWARE_BUILD:-0}" == "1" ]]; then
  bash "${repository}/scripts/build-components.sh"
  bash "${repository}/scripts/generate-checksums.sh"
fi

python3 - "${MIDDLEWARE_LOCK}" "${repository}/compatibility.toml" <<'PY'
import sys
import tomllib
from pathlib import Path

with open(sys.argv[1], "rb") as source:
    integration = tomllib.load(source)
with open(sys.argv[2], "rb") as source:
    middleware_document = tomllib.load(source)
middleware = middleware_document["compatibility"]
middleware_release = middleware_document["release"]
with (Path(sys.argv[2]).parent / "Cargo.toml").open("rb") as source:
    workspace = tomllib.load(source)["workspace"]["package"]

expected = {
    "wasi_http": integration["wasi_http"],
    "wasip3": f'{integration["wasip3_version"]}+wasi-{integration["wasi_http"]}',
    "wasmtime": integration["wasmtime_version"],
    "wasm_tools": integration["wasm_tools_version"],
    "wac": integration["wac_cli_version"],
    "oha": integration["oha_version"],
}
for key, value in expected.items():
    if middleware.get(key) != value:
        raise SystemExit(
            f"middleware compatibility mismatch for {key}: "
            f"expected {value}, found {middleware.get(key)}"
        )
target_version = integration["middleware"]["version"]
if middleware_release.get("version") != target_version or workspace.get("version") != target_version:
    raise SystemExit(
        "middleware release target mismatch: "
        f"expected {target_version}, found release={middleware_release.get('version')} "
        f"workspace={workspace.get('version')}"
    )
PY

expected_revision="$(middleware_lock_value source_revision)"
actual_revision="$(git -C "${repository}" rev-parse HEAD)"
[[ "${actual_revision}" == "${expected_revision}" ]] || {
  echo "middleware source revision mismatch: expected ${expected_revision}, found ${actual_revision}" >&2
  exit 1
}
baseline_revision="$(middleware_lock_value baseline_revision)"
git -C "${repository}" merge-base --is-ancestor "${baseline_revision}" "${actual_revision}" || {
  echo "middleware source revision does not descend from baseline ${baseline_revision}" >&2
  exit 1
}
if [[ -n "$(git -C "${repository}" status --porcelain)" ]]; then
  echo "middleware checkout must be clean before artifact synchronization" >&2
  exit 1
fi

python3 "${MIDDLEWARE_ROOT}/scripts/verify-artifact-set.py" middleware \
  --repository "${repository}" \
  --cosign "${cosign_bin}"

mkdir -p "${destination}"
components=(request-id security-headers cors authn-policy secure-defaults)
for component in "${components[@]}"; do
  source_path="${artifact_root}/components/${component}.wasm"
  [[ -f "${source_path}" ]] || {
    echo "middleware artifact is unavailable: ${source_path}" >&2
    exit 2
  }
  expected="$(awk -v path="components/${component}.wasm" '$2 == path { print $1 }' "${artifact_root}/SHA256SUMS")"
  actual="$(sha256_for_file "${source_path}")"
  [[ -n "${expected}" && "${actual}" == "${expected}" ]] || {
    echo "middleware checksum mismatch: ${component}" >&2
    exit 1
  }
  install -m 0644 "${source_path}" "${destination}/${component}.wasm"
  "${wasm_tools_bin}" validate --features component-model,cm-async "${destination}/${component}.wasm"
  wit="$("${wasm_tools_bin}" component wit "${destination}/${component}.wasm")"
  printf '%s\n' "${wit}" | rg -q 'import wasi:http/handler@0\.3\.0;' || {
    echo "middleware does not import final wasi:http/handler@0.3.0: ${component}" >&2
    exit 1
  }
  printf '%s\n' "${wit}" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
    echo "middleware does not export final wasi:http/handler@0.3.0: ${component}" >&2
    exit 1
  }
  if printf '%s\n' "${wit}" | rg -q '0\.3\.0-rc'; then
    echo "middleware still imports an RC WASI HTTP interface: ${component}" >&2
    exit 1
  fi
done

broker="${artifact_root}/test-components/mock-authn-broker.wasm"
[[ -f "${broker}" ]] || {
  echo "authentication broker fixture is unavailable: ${broker}" >&2
  exit 2
}
expected="$(awk '$2 == "test-components/mock-authn-broker.wasm" { print $1 }' "${artifact_root}/SHA256SUMS")"
actual="$(sha256_for_file "${broker}")"
[[ -n "${expected}" && "${actual}" == "${expected}" ]] || {
  echo "authentication broker checksum mismatch" >&2
  exit 1
}
install -m 0644 "${broker}" "${destination}/mock-authn-broker.wasm"
"${wasm_tools_bin}" validate --features component-model,cm-async "${destination}/mock-authn-broker.wasm"
broker_wit="$("${wasm_tools_bin}" component wit "${destination}/mock-authn-broker.wasm")"
printf '%s\n' "${broker_wit}" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
  echo "authentication broker does not export final wasi:http/handler@0.3.0" >&2
  exit 1
}

echo "verified middleware artifacts in ${destination}"

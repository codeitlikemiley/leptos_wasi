#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/authz-common.sh"
source "${SCRIPT_DIR}/middleware-common.sh"

repository="$(authz_repository)"
artifact_root="${repository}/artifacts"
checksum_manifest="${artifact_root}/RELEASE-SHA256SUMS"
destination="${LEPTOS_WASI_ROOT}/tests/authz-artifacts"
wasm_tools_bin="$(resolve_middleware_tool WASM_TOOLS_BIN wasm-tools "$(middleware_lock_value wasm_tools_version)")"
cosign_bin="$(resolve_middleware_tool COSIGN_BIN cosign "$(middleware_lock_value cosign_version)")"

if [[ "${WASI_AUTHZ_BUILD:-0}" == "1" ]]; then
  bash "${repository}/scripts/build-components.sh"
  bash "${repository}/scripts/generate-checksums.sh"
  bash "${repository}/scripts/check-component-contracts.sh"
fi

bash "${SCRIPT_DIR}/check-authz-companion.sh"
python3 "${LEPTOS_WASI_ROOT}/scripts/verify-artifact-set.py" authorization \
  --repository "${repository}" \
  --cosign "${cosign_bin}"
mkdir -p "${destination}"
while IFS= read -r component; do
  source_path="${artifact_root}/components/${component}.wasm"
  [[ -f "${source_path}" ]] || {
    echo "authorization artifact is unavailable: ${source_path}" >&2
    exit 2
  }
  expected="$(awk -v path="components/${component}.wasm" '$2 == path { print $1 }' "${checksum_manifest}")"
  actual="$(sha256_for_file "${source_path}")"
  [[ -n "${expected}" && "${actual}" == "${expected}" ]] || {
    echo "authorization checksum mismatch: ${component}" >&2
    exit 1
  }
  install -m 0644 "${source_path}" "${destination}/${component}.wasm"
  "${wasm_tools_bin}" validate --features component-model,cm-async "${destination}/${component}.wasm"
  wit="$("${wasm_tools_bin}" component wit "${destination}/${component}.wasm")"
  printf '%s\n' "${wit}" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
    echo "authorization component does not export final wasi:http/handler@0.3.0: ${component}" >&2
    exit 1
  }
  if printf '%s\n' "${wit}" | rg -q '0\.3\.0-rc'; then
    echo "authorization component still imports an RC WASI HTTP interface: ${component}" >&2
    exit 1
  fi
  if [[ "${component}" == "authz-http-pep" ]]; then
    printf '%s\n' "${wit}" | rg -q 'import wasi:http/handler@0\.3\.0;' || {
      echo "authorization PEP is missing its downstream final-WASI handler" >&2
      exit 1
    }
  elif printf '%s\n' "${wit}" | rg -q 'import wasi:http/handler@0\.3\.0;'; then
    echo "terminal authorization PDP unexpectedly imports a downstream handler: ${component}" >&2
    exit 1
  fi
done < <(authz_components)

echo "verified authorization artifacts in ${destination}"

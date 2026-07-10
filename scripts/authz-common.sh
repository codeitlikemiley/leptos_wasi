#!/usr/bin/env bash

set -euo pipefail

LEPTOS_WASI_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUTHZ_LOCK="${LEPTOS_WASI_ROOT}/tests/middleware/components.lock.toml"

authz_lock_value() {
  python3 - "${AUTHZ_LOCK}" "$1" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    lock = tomllib.load(source)
value = lock["authorization"][sys.argv[2]]
if not isinstance(value, str) or not value:
    raise SystemExit(f"invalid authorization lock value: {sys.argv[2]}")
print(value)
PY
}

authz_repository() {
  if [[ -n "${WASI_AUTHZ_DIR:-}" ]]; then
    printf '%s\n' "${WASI_AUTHZ_DIR}"
    return
  fi
  local relative
  relative="$(authz_lock_value relative_path)"
  printf '%s/%s\n' "$(dirname "${LEPTOS_WASI_ROOT}")" "${relative#../}"
}

authz_components() {
  python3 - "${AUTHZ_LOCK}" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    components = tomllib.load(source)["authorization"]["components"]
if not isinstance(components, list) or not components:
    raise SystemExit("authorization components must be a non-empty array")
for component in components:
    if not isinstance(component, str) or not component:
        raise SystemExit("authorization component names must be non-empty strings")
    print(component)
PY
}

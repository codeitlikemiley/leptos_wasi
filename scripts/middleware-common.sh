#!/usr/bin/env bash

set -euo pipefail

MIDDLEWARE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIDDLEWARE_LOCK="${MIDDLEWARE_ROOT}/tests/middleware/components.lock.toml"

middleware_lock_value() {
  local key="$1"
  local value
  value="$(sed -nE "s/^${key}[[:space:]]*=[[:space:]]*\"([^\"]+)\".*/\1/p" "${MIDDLEWARE_LOCK}" | head -n 1)"
  if [[ -z "${value}" ]]; then
    echo "missing middleware compatibility key: ${key}" >&2
    return 1
  fi
  printf '%s\n' "${value}"
}

require_middleware_command() {
  local command_name="$1"
  if [[ "${command_name}" == */* ]]; then
    [[ -x "${command_name}" ]] || {
      echo "required command is not executable: ${command_name}" >&2
      return 1
    }
  else
    command -v "${command_name}" >/dev/null 2>&1 || {
      echo "required command is not installed: ${command_name}" >&2
      return 1
    }
  fi
}

resolve_middleware_tool() {
  local environment_name="$1"
  local command_name="$2"
  local expected_version="$3"
  local configured="${!environment_name:-}"
  local cache_root="${LEPTOS_WASI_TOOL_ROOT:-${HOME}/.cache/leptos-wasi-tools}"
  local cached="${cache_root}/${command_name}-${expected_version}/${command_name}"
  local selected

  if [[ -n "${configured}" ]]; then
    selected="${configured}"
  elif [[ -x "${cached}" ]]; then
    selected="${cached}"
  else
    selected="${command_name}"
  fi

  require_middleware_command "${selected}"
  local actual
  if [[ "${command_name}" == "cosign" ]]; then
    actual="$("${selected}" version 2>&1)"
  else
    actual="$("${selected}" --version 2>&1)"
  fi
  if [[ "${actual}" != *"${expected_version}"* ]]; then
    echo "${command_name} version mismatch; expected ${expected_version}, found: ${actual}" >&2
    return 1
  fi
  printf '%s\n' "${selected}"
}

middleware_repository() {
  if [[ -n "${WASI_HTTP_MIDDLEWARE_DIR:-}" ]]; then
    printf '%s\n' "${WASI_HTTP_MIDDLEWARE_DIR}"
    return
  fi
  local relative
  relative="$(middleware_lock_value relative_path)"
  printf '%s/%s\n' "$(dirname "${MIDDLEWARE_ROOT}")" "${relative#../}"
}

sha256_for_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${path}" | cut -d ' ' -f 1
  else
    shasum -a 256 "${path}" | cut -d ' ' -f 1
  fi
}

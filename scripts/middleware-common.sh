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

# A substring test accepts a longer version that merely starts with the pinned
# one - `4.0.2` matches a reported `4.0.21`, `0.10.1` matches `0.10.10` - which
# defeats the point of recording exact revisions in components.lock.toml.
# Require the pinned version to appear as a whole version string.
# A git revision is compared by short prefix on purpose: the lock records a
# full 40-character SHA while the binary reports a shorter one, so `c34c584`
# must still match a reported `c34c584d`. That is a prefix test, not a version
# test, and it must not use the whole-version rule below.
revision_string_matches() {
  local expected_prefix="$1"
  local reported="$2"
  [[ "${reported}" == *"${expected_prefix}"* ]]
}

version_string_matches() {
  local expected="$1"
  local reported="$2"
  local escaped
  escaped="$(sed 's/[.[\*^$()+?{|]/\\&/g' <<<"${expected}")"
  grep -qE "(^|[^0-9A-Za-z.-])${escaped}([^0-9A-Za-z.-]|\$)" <<<"${reported}"
}

resolve_middleware_tool() {
  local environment_name="$1"
  local command_name="$2"
  local expected_version="$3"
  local configured="${!environment_name:-}"
  local repo_local="${MIDDLEWARE_ROOT}/target/tools/${command_name}-${expected_version}/${command_name}"
  local legacy_cache="${LEPTOS_WASI_TOOL_ROOT:-${HOME}/.cache/leptos-wasi-tools}/${command_name}-${expected_version}/${command_name}"
  local selected

  if [[ -n "${configured}" ]]; then
    selected="${configured}"
  elif command -v "${command_name}" >/dev/null 2>&1 \
    && tool_version_matches "${command_name}" "${expected_version}"; then
    selected="${command_name}"
  elif [[ -x "${repo_local}" ]]; then
    selected="${repo_local}"
  elif [[ -x "${legacy_cache}" ]]; then
    selected="${legacy_cache}"
  else
    echo "${command_name} ${expected_version} is required; install it or set ${environment_name}" >&2
    return 1
  fi

  require_middleware_command "${selected}"
  local actual
  if [[ "${command_name}" == "cosign" ]]; then
    actual="$("${selected}" version 2>&1)"
  else
    actual="$("${selected}" --version 2>&1)"
  fi
  if ! version_string_matches "${expected_version}" "${actual}"; then
    echo "${command_name} version mismatch; expected ${expected_version}, found: ${actual}" >&2
    return 1
  fi
  printf '%s\n' "${selected}"
}

tool_version_matches() {
  local command_name="$1"
  local expected_version="$2"
  local actual
  if [[ "${command_name}" == "cosign" ]]; then
    actual="$("${command_name}" version 2>&1 || true)"
  else
    actual="$("${command_name}" --version 2>&1 || true)"
  fi
  version_string_matches "${expected_version}" "${actual}"
}

resolve_spin_main_tool() {
  local configured="${SPIN_BIN:-}"
  local revision
  revision="$(middleware_lock_value spin_main_revision)"
  local short_revision="${revision:0:7}"
  local repo_local="${MIDDLEWARE_ROOT}/target/tools/spin-${revision}/spin"
  local selected

  if [[ -n "${configured}" ]]; then
    selected="${configured}"
  elif command -v spin >/dev/null 2>&1 \
    && revision_string_matches "${short_revision}" "$(spin --version 2>&1 || true)"; then
    selected="spin"
  elif [[ -x "${repo_local}" ]]; then
    selected="${repo_local}"
  else
    echo "pinned Spin main ${revision} is required; run scripts/bootstrap-spin-main.sh or set SPIN_BIN" >&2
    return 1
  fi

  require_middleware_command "${selected}"
  revision_string_matches "${short_revision}" "$("${selected}" --version 2>&1 || true)" || {
    echo "Spin revision mismatch; expected ${revision}, found: $("${selected}" --version 2>&1)" >&2
    return 1
  }
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

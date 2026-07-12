#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_ROOT="${ROOT}/tests/authz-lifecycle-wasip2"
FAULT_FIXTURE_ROOT="${ROOT}/tests/authz-lifecycle"
LOCK="${ROOT}/tests/middleware/components.lock.toml"
AUTHZ_REPOSITORY="${WASI_AUTH_DIR:-${WASI_AUTHZ_DIR:-$(dirname "${ROOT}")/wasi-auth}}"
ALLOW_DIRTY="${AUTHZ_WASIP2_LIFECYCLE_ALLOW_DIRTY_COMPANION:-0}"
RUST_VERSION="1.93.0"
CEDAR_TOKEN="wasip2-cedar-transport-token"
FAULT_TOKEN="lifecycle-pdp-transport-token"

lock_value() {
    python3 - "${LOCK}" "$1" "$2" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    document = tomllib.load(source)
value = document[sys.argv[2]][sys.argv[3]] if len(sys.argv) == 4 else document[sys.argv[2]]
if not isinstance(value, str) or not value:
    raise SystemExit("invalid lifecycle compatibility lock value")
print(value)
PY
}

root_lock_value() {
    python3 - "${LOCK}" "$1" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    value = tomllib.load(source)[sys.argv[2]]
if not isinstance(value, str) or not value:
    raise SystemExit(f"invalid lifecycle compatibility lock value: {sys.argv[2]}")
print(value)
PY
}

resolve_tool() {
    local environment_name="$1"
    local command_name="$2"
    local expected_version="$3"
    local configured="${!environment_name:-}"
    local cached="${HOME}/.cache/leptos-wasi-tools/${command_name}-${expected_version}/${command_name}"
    local selected
    if [[ -n "${configured}" ]]; then
        selected="${configured}"
    elif [[ -x "${cached}" ]]; then
        selected="${cached}"
    else
        selected="$(command -v "${command_name}")"
    fi
    local actual
    actual="$("${selected}" --version 2>&1)"
    [[ "${actual}" == *"${expected_version}"* ]] || {
        echo "error: ${command_name} version mismatch; expected ${expected_version}, found ${actual}" >&2
        return 1
    }
    printf '%s\n' "${selected}"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

available_port() {
    python3 -c 'import socket; stream=socket.socket(); stream.bind(("127.0.0.1", 0)); print(stream.getsockname()[1]); stream.close()'
}

wait_for_status() {
    local url="$1"
    local expected="$2"
    local method="${3:-GET}"
    local status
    for _ in $(seq 1 120); do
        status="$(curl --silent --max-time 1 --request "${method}" --output /dev/null --write-out '%{http_code}' "${url}" || true)"
        if [[ "${status}" == "${expected}" ]]; then
            return 0
        fi
        sleep 0.1
    done
    echo "error: ${url} did not become ready with status ${expected}" >&2
    return 1
}

expected_revision="$(lock_value authorization source_revision)"
actual_revision="$(git -C "${AUTHZ_REPOSITORY}" rev-parse HEAD)"
if [[ "${actual_revision}" != "${expected_revision}" ]] \
    || [[ -n "$(git -C "${AUTHZ_REPOSITORY}" status --porcelain)" ]]; then
    if [[ "${ALLOW_DIRTY}" == "1" ]]; then
        echo "warning: wasi-authz is not exact/clean; this run is not release evidence" >&2
    else
        echo "error: wasi-authz must be clean at ${expected_revision}; found ${actual_revision}" >&2
        exit 1
    fi
fi

RUSTC_VERSION="$(rustup run "${RUST_VERSION}" rustc --version)"
[[ "${RUSTC_VERSION}" == "rustc ${RUST_VERSION}"* ]] || {
    echo "error: Rust ${RUST_VERSION} is required; found ${RUSTC_VERSION}" >&2
    exit 1
}
WASMTIME_BIN="$(resolve_tool WASMTIME_BIN wasmtime "$(root_lock_value wasmtime_version)")"
WASM_TOOLS_BIN="$(resolve_tool WASM_TOOLS_BIN wasm-tools "$(root_lock_value wasm_tools_version)")"

native_target="$(rustup run "${RUST_VERSION}" rustc -vV | sed -n 's/^host: //p')"
native_suffix=""
[[ "${native_target}" == *windows* ]] && native_suffix=".exe"
CEDAR_PDP="${AUTHZ_REPOSITORY}/artifacts/native/cedar-pdp-${native_target}${native_suffix}"
[[ -x "${CEDAR_PDP}" ]] || {
    echo "error: authenticated native Cedar PDP is unavailable: ${CEDAR_PDP}" >&2
    exit 2
}
native_relative="native/$(basename "${CEDAR_PDP}")"
expected_native="$(awk -v path="${native_relative}" '$2 == path { print $1 }' "${AUTHZ_REPOSITORY}/artifacts/native/SHA256SUMS")"
actual_native="$(sha256_file "${CEDAR_PDP}")"
[[ -n "${expected_native}" && "${actual_native}" == "${expected_native}" ]] || {
    echo "error: native Cedar PDP checksum mismatch" >&2
    exit 1
}

rustup run "${RUST_VERSION}" cargo build \
    --manifest-path "${FAULT_FIXTURE_ROOT}/Cargo.toml" \
    --locked \
    --release \
    --target wasm32-wasip2 \
    --package leptos-wasi-authz-fault-pdp
FAULT_PDP="${FAULT_FIXTURE_ROOT}/target/wasm32-wasip2/release/leptos_wasi_authz_fault_pdp.wasm"

rustup run "${RUST_VERSION}" cargo build \
    --manifest-path "${FIXTURE_ROOT}/Cargo.toml" \
    --locked \
    --release \
    --target wasm32-wasip2
APP_COMPONENT="${FIXTURE_ROOT}/target/wasm32-wasip2/release/leptos_wasi_authz_wasip2_lifecycle.wasm"

"${WASM_TOOLS_BIN}" validate --features component-model "${APP_COMPONENT}"
APP_WIT="$(${WASM_TOOLS_BIN} component wit "${APP_COMPONENT}")"
printf '%s\n' "${APP_WIT}" | rg -q 'export wasi:http/incoming-handler@0\.2\.' || {
    echo "error: lifecycle app is not a WASIp2 HTTP incoming component" >&2
    exit 1
}
"${WASM_TOOLS_BIN}" validate --features component-model,cm-async "${FAULT_PDP}"
FAULT_WIT="$(${WASM_TOOLS_BIN} component wit "${FAULT_PDP}")"
printf '%s\n' "${FAULT_WIT}" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
    echo "error: fault PDP does not export final wasi:http/handler@0.3.0" >&2
    exit 1
}

cedar_port="$(available_port)"
fault_port="$(available_port)"
app_port="$(available_port)"
CEDAR_URL="http://127.0.0.1:${cedar_port}/access/v1/evaluation"
FAULT_URL="http://127.0.0.1:${fault_port}/access/v1/evaluation"
APP_URL="http://127.0.0.1:${app_port}"
TEMPORARY="$(mktemp -d "${TMPDIR:-/tmp}/leptos-authz-p2-lifecycle.XXXXXX")"
cedar_pid=""
fault_pid=""
app_pid=""

cleanup() {
    local status=$?
    trap - EXIT
    for pid in "${app_pid}" "${fault_pid}" "${cedar_pid}"; do
        [[ -n "${pid}" ]] && kill "${pid}" >/dev/null 2>&1 || true
    done
    for pid in "${app_pid}" "${fault_pid}" "${cedar_pid}"; do
        [[ -n "${pid}" ]] && wait "${pid}" >/dev/null 2>&1 || true
    done
    if [[ "${AUTHZ_WASIP2_LIFECYCLE_KEEP_TEMP:-0}" == "1" ]]; then
        echo "WASIp2 lifecycle temporary files: ${TEMPORARY}" >&2
    elif [[ "${status}" == "0" ]]; then
        rm -rf "${TEMPORARY}"
    else
        tail -n 120 "${TEMPORARY}/cedar.log" "${TEMPORARY}/fault.log" "${TEMPORARY}/app.log" >&2 || true
        rm -rf "${TEMPORARY}"
    fi
    exit "${status}"
}
trap cleanup EXIT

WASI_AUTHZ_CEDAR_LISTEN="127.0.0.1:${cedar_port}" \
WASI_AUTHZ_CEDAR_WORKERS=2 \
WASI_AUTHZ_CEDAR_POLICY_PATH="${AUTHZ_REPOSITORY}/fixtures/cedar/http_policy.cedar" \
WASI_AUTHZ_CEDAR_SCHEMA_PATH="${AUTHZ_REPOSITORY}/fixtures/cedar/http_schema.json" \
WASI_AUTHZ_CEDAR_ENTITIES_PATH="${AUTHZ_REPOSITORY}/fixtures/cedar/http_entities.json" \
WASI_AUTHZ_CEDAR_POLICY_REVISION=http-policy-1 \
WASI_AUTHZ_PDP_BEARER_TOKEN="${CEDAR_TOKEN}" \
    "${CEDAR_PDP}" >"${TEMPORARY}/cedar.log" 2>&1 &
cedar_pid=$!

"${WASMTIME_BIN}" serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --addr "127.0.0.1:${fault_port}" \
    "${FAULT_PDP}" >"${TEMPORARY}/fault.log" 2>&1 &
fault_pid=$!

cedar_ready=false
for _ in $(seq 1 120); do
    status="$(curl --silent --max-time 2 \
        --header 'content-type: application/json' \
        --data-binary "@${AUTHZ_REPOSITORY}/fixtures/cedar/http_public_request.json" \
        --output /dev/null \
        --write-out '%{http_code}' \
        "${CEDAR_URL}" || true)"
    if [[ "${status}" == "401" ]]; then
        cedar_ready=true
        break
    fi
    sleep 0.1
done
[[ "${cedar_ready}" == "true" ]] || {
    echo "error: authenticated native Cedar PDP did not become ready" >&2
    exit 1
}
wait_for_status "${FAULT_URL}" 401 POST
fault_warm_status="$(curl --silent --show-error --max-time 2 \
    --request POST \
    --header "authorization: Bearer ${FAULT_TOKEN}" \
    --header 'content-type: application/json' \
    --data '{"context":{"wasi_authz":{"request_id":"lifecycle-pdp-warmup"}}}' \
    --output /dev/null \
    --write-out '%{http_code}' \
    "${FAULT_URL}")"
[[ "${fault_warm_status}" == "200" ]] || {
    echo "error: fault PDP warmup returned ${fault_warm_status}" >&2
    exit 1
}

"${WASMTIME_BIN}" serve \
    -S cli=y \
    -S http=y \
    -S inherit-network=y \
    --env "CEDAR_PDP_URL=${CEDAR_URL}" \
    --env "CEDAR_PDP_TOKEN=${CEDAR_TOKEN}" \
    --env "FAULT_PDP_URL=${FAULT_URL}" \
    --env "FAULT_PDP_TOKEN=${FAULT_TOKEN}" \
    --env "P2_AUTHZ_TIMEOUT_MS=500" \
    --addr "127.0.0.1:${app_port}" \
    "${APP_COMPONENT}" >"${TEMPORARY}/app.log" 2>&1 &
app_pid=$!

wait_for_status "${APP_URL}/queue-depth" 204
PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" \
    --base-url "${APP_URL}"

for sentinel in \
    "${CEDAR_TOKEN}" \
    "${FAULT_TOKEN}" \
    'wasip2-secret-cookie-sentinel' \
    'wasip2-secret-query-sentinel' \
    'middleware-secret-issuer-sentinel' \
    'user-1'; do
    for log in "${TEMPORARY}/cedar.log" "${TEMPORARY}/fault.log" "${TEMPORARY}/app.log"; do
        if grep -Fq -- "${sentinel}" "${log}"; then
            echo "error: WASIp2 lifecycle log disclosed credential, cookie, query, or identity data" >&2
            echo "log: ${log}" >&2
            exit 1
        fi
    done
done

echo "WASIp2 authorization lifecycle passed on Wasmtime $(root_lock_value wasmtime_version)"

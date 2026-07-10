#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_ROOT="${ROOT}/tests/authz-lifecycle"
LOCK="${ROOT}/tests/middleware/components.lock.toml"
MIDDLEWARE_REPOSITORY="${WASI_HTTP_MIDDLEWARE_DIR:-$(dirname "${ROOT}")/wasi-http-middleware}"
AUTHZ_REPOSITORY="${WASI_AUTHZ_DIR:-$(dirname "${ROOT}")/wasi-authz}"
ALLOW_DIRTY="${AUTHZ_LIFECYCLE_ALLOW_DIRTY_COMPANIONS:-0}"

available_port() {
    python3 -c 'import socket; stream=socket.socket(); stream.bind(("127.0.0.1", 0)); print(stream.getsockname()[1]); stream.close()'
}

APP_PORT="${AUTHZ_LIFECYCLE_APP_PORT:-$(available_port)}"
BROKER_PORT="${AUTHZ_LIFECYCLE_BROKER_PORT:-$(available_port)}"
PDP_PORT="${AUTHZ_LIFECYCLE_PDP_PORT:-$(available_port)}"
APP_ADDRESS="127.0.0.1:${APP_PORT}"
BROKER_ADDRESS="127.0.0.1:${BROKER_PORT}"
PDP_ADDRESS="127.0.0.1:${PDP_PORT}"
APP_URL="http://${APP_ADDRESS}"
BROKER_URL="http://${BROKER_ADDRESS}/authenticate"
PDP_URL="http://${PDP_ADDRESS}"
PDP_EVALUATION_URL="${PDP_URL}/access/v1/evaluation"
PDP_TOKEN="lifecycle-pdp-transport-token"
RUST_VERSION="1.93.0"

lock_value() {
    python3 - "${LOCK}" "$1" "$2" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    value = tomllib.load(source)[sys.argv[2]][sys.argv[3]]
if not isinstance(value, str) or not value:
    raise SystemExit(f"invalid compatibility lock value: {sys.argv[2]}.{sys.argv[3]}")
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
    raise SystemExit(f"invalid compatibility lock value: {sys.argv[2]}")
print(value)
PY
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
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

check_companion() {
    local repository="$1"
    local section="$2"
    local label="$3"
    [[ -f "${repository}/Cargo.toml" ]] || {
        echo "error: ${label} checkout is unavailable: ${repository}" >&2
        return 1
    }
    local expected_revision
    local actual_revision
    expected_revision="$(lock_value "${section}" source_revision)"
    actual_revision="$(git -C "${repository}" rev-parse HEAD)"
    if [[ "${actual_revision}" != "${expected_revision}" ]] \
        || [[ -n "$(git -C "${repository}" status --porcelain)" ]]; then
        if [[ "${ALLOW_DIRTY}" == "1" ]]; then
            echo "warning: ${label} is not exact/clean; this run is not release evidence" >&2
        else
            echo "error: ${label} must be clean at ${expected_revision}; found ${actual_revision}" >&2
            return 1
        fi
    fi
}

verify_artifact() {
    local repository="$1"
    local relative_path="$2"
    local artifact="${repository}/artifacts/${relative_path}"
    [[ -f "${artifact}" ]] || {
        echo "error: companion artifact is missing: ${artifact}" >&2
        return 1
    }
    local expected
    local actual
    expected="$(awk -v path="${relative_path}" '$2 == path { print $1 }' "${repository}/artifacts/SHA256SUMS")"
    actual="$(sha256_file "${artifact}")"
    if [[ -z "${expected}" || "${actual}" != "${expected}" ]]; then
        if [[ "${ALLOW_DIRTY}" == "1" ]]; then
            echo "warning: checksum drift for ${relative_path}; this run is not release evidence" >&2
        else
            echo "error: companion artifact checksum mismatch: ${relative_path}" >&2
            return 1
        fi
    fi
    printf '%s\n' "${artifact}"
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

check_companion "${MIDDLEWARE_REPOSITORY}" middleware wasi-http-middleware
check_companion "${AUTHZ_REPOSITORY}" authorization wasi-authz
RUSTC_VERSION="$(rustup run "${RUST_VERSION}" rustc --version)"
[[ "${RUSTC_VERSION}" == "rustc ${RUST_VERSION}"* ]] || {
    echo "error: Rust ${RUST_VERSION} is required; found ${RUSTC_VERSION}" >&2
    exit 1
}

WASMTIME_BIN="$(resolve_tool WASMTIME_BIN wasmtime "$(root_lock_value wasmtime_version)")"
WASM_TOOLS_BIN="$(resolve_tool WASM_TOOLS_BIN wasm-tools "$(root_lock_value wasm_tools_version)")"
WAC_BIN="$(resolve_tool WAC_BIN wac "$(root_lock_value wac_cli_version)")"

REQUEST_ID_COMPONENT="$(verify_artifact "${MIDDLEWARE_REPOSITORY}" components/request-id.wasm)"
AUTHN_COMPONENT="$(verify_artifact "${MIDDLEWARE_REPOSITORY}" components/authn-policy.wasm)"
BROKER_COMPONENT="$(verify_artifact "${MIDDLEWARE_REPOSITORY}" test-components/mock-authn-broker.wasm)"
if [[ -n "${AUTHZ_LIFECYCLE_AUTHZ_COMPONENT:-}" ]]; then
    [[ "${ALLOW_DIRTY}" == "1" ]] || {
        echo "error: an authorization component override is development-only" >&2
        exit 1
    }
    AUTHZ_COMPONENT="${AUTHZ_LIFECYCLE_AUTHZ_COMPONENT}"
    [[ -f "${AUTHZ_COMPONENT}" ]] || {
        echo "error: authorization component override is missing: ${AUTHZ_COMPONENT}" >&2
        exit 1
    }
    echo "warning: using an authorization component override; this run is not release evidence" >&2
else
    AUTHZ_COMPONENT="$(verify_artifact "${AUTHZ_REPOSITORY}" components/authz-http-pep.wasm)"
fi

rustup run "${RUST_VERSION}" cargo build \
    --manifest-path "${FIXTURE_ROOT}/Cargo.toml" \
    --locked \
    --release \
    --target wasm32-wasip2
FIXTURE_TARGET="${FIXTURE_ROOT}/target/wasm32-wasip2/release"
FAULT_PDP_COMPONENT="${FIXTURE_TARGET}/leptos_wasi_authz_fault_pdp.wasm"
PROBE_COMPONENT="${FIXTURE_TARGET}/leptos_wasi_authz_downstream_probe.wasm"

TEST_APP_TARGET="${FIXTURE_ROOT}/target/test-app"
CARGO_TARGET_DIR="${TEST_APP_TARGET}" LEPTOS_OUTPUT_NAME=test-app \
    rustup run "${RUST_VERSION}" cargo build \
    --locked \
    --manifest-path "${ROOT}/tests/test-app/Cargo.toml" \
    --target wasm32-wasip2 \
    --release \
    --no-default-features \
    --features wasip3,islands-router
APP_COMPONENT="${TEST_APP_TARGET}/wasm32-wasip2/release/test_app.wasm"

for component in \
    "${FAULT_PDP_COMPONENT}" \
    "${PROBE_COMPONENT}" \
    "${APP_COMPONENT}" \
    "${REQUEST_ID_COMPONENT}" \
    "${AUTHN_COMPONENT}" \
    "${AUTHZ_COMPONENT}"; do
    "${WASM_TOOLS_BIN}" validate --features component-model,cm-async "${component}"
    wit="$(${WASM_TOOLS_BIN} component wit "${component}")"
    printf '%s\n' "${wit}" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
        echo "error: component lacks final wasi:http/handler@0.3.0 export: ${component}" >&2
        exit 1
    }
    if printf '%s\n' "${wit}" | rg -q '0\.3\.0-rc'; then
        echo "error: component retains a WASI 0.3 release-candidate import: ${component}" >&2
        exit 1
    fi
done
for component in "${PROBE_COMPONENT}" "${REQUEST_ID_COMPONENT}" "${AUTHN_COMPONENT}" "${AUTHZ_COMPONENT}"; do
    "${WASM_TOOLS_BIN}" component wit "${component}" \
        | rg -q 'import wasi:http/handler@0\.3\.0;' || {
        echo "error: middleware lacks its downstream final-WASI import: ${component}" >&2
        exit 1
    }
done
if "${WASM_TOOLS_BIN}" component wit "${FAULT_PDP_COMPONENT}" \
    | rg -q 'import wasi:http/handler@0\.3\.0;'; then
    echo "error: terminal fault PDP unexpectedly imports a downstream handler" >&2
    exit 1
fi

TEMPORARY="$(mktemp -d "${TMPDIR:-/tmp}/leptos-authz-lifecycle.XXXXXX")"
COMPOSED="${TEMPORARY}/leptos-authz-lifecycle.wasm"
CURRENT="${APP_COMPONENT}"
index=0
for component in "${PROBE_COMPONENT}" "${AUTHZ_COMPONENT}" "${AUTHN_COMPONENT}" "${REQUEST_ID_COMPONENT}"; do
    stage="${TEMPORARY}/stage-${index}.wasm"
    "${WAC_BIN}" plug --plug "${CURRENT}" "${component}" --output "${stage}"
    CURRENT="${stage}"
    index=$((index + 1))
done
install -m 0644 "${CURRENT}" "${COMPOSED}"
"${WASM_TOOLS_BIN}" validate --features component-model,cm-async "${COMPOSED}"

app_pid=""
broker_pid=""
pdp_pid=""
cleanup() {
    for pid in "${app_pid}" "${pdp_pid}" "${broker_pid}"; do
        [[ -n "${pid}" ]] && kill "${pid}" >/dev/null 2>&1 || true
    done
    for pid in "${app_pid}" "${pdp_pid}" "${broker_pid}"; do
        [[ -n "${pid}" ]] && wait "${pid}" >/dev/null 2>&1 || true
    done
    if [[ "${AUTHZ_LIFECYCLE_KEEP_TEMP:-0}" == "1" ]]; then
        echo "authorization lifecycle temporary files: ${TEMPORARY}" >&2
    else
        rm -rf "${TEMPORARY}"
    fi
}
trap cleanup EXIT

"${WASMTIME_BIN}" serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    --addr "${BROKER_ADDRESS}" \
    "${BROKER_COMPONENT}" \
    >"${TEMPORARY}/broker.log" 2>&1 &
broker_pid=$!

start_pdp() {
    "${WASMTIME_BIN}" serve \
        -W component-model-async=y \
        -S p3=y \
        -S cli=y \
        -S http=y \
        --addr "${PDP_ADDRESS}" \
        "${FAULT_PDP_COMPONENT}" \
        >>"${TEMPORARY}/pdp.log" 2>&1 &
    pdp_pid=$!
    wait_for_status "${PDP_EVALUATION_URL}" 401 POST
    warm_status="$(curl --silent --show-error --max-time 2 \
        --request POST \
        --header "authorization: Bearer ${PDP_TOKEN}" \
        --header 'content-type: application/json' \
        --data '{"context":{"wasi_authz":{"request_id":"lifecycle-pdp-warmup"}}}' \
        --output /dev/null \
        --write-out '%{http_code}' \
        "${PDP_EVALUATION_URL}")"
    [[ "${warm_status}" == "200" ]] || {
        echo "error: fault PDP warmup returned ${warm_status}" >&2
        return 1
    }
}

start_pdp
wait_for_status "${BROKER_URL}" 401 POST

"${WASMTIME_BIN}" serve \
    -W component-model-async=y \
    -S p3=y \
    -S cli=y \
    -S http=y \
    -S inherit-network=y \
    --env "WASI_MIDDLEWARE_AUTHN_BROKER_URL=${BROKER_URL}" \
    --env "WASI_MIDDLEWARE_AUTHN_TIMEOUT_MS=1000" \
    --env "WASI_MIDDLEWARE_AUTHN_MODE=optional" \
    --env "WASI_MIDDLEWARE_SERVICE_ID=leptos-wasi-test-app" \
    --env "WASI_MIDDLEWARE_AUTHN_AUDIENCES=api://leptos-wasi-test-app" \
    --env "WASI_MIDDLEWARE_AUTHN_MAX_IN_FLIGHT=64" \
    --env "WASI_MIDDLEWARE_AUTHN_ALLOW_INSECURE_LOOPBACK=true" \
    --env "WASI_AUTHZ_SERVICE_ID=leptos-wasi-test-app" \
    --env "WASI_AUTHZ_ENDPOINT=${PDP_EVALUATION_URL}" \
    --env "WASI_AUTHZ_TIMEOUT_MS=500" \
    --env "WASI_AUTHZ_ALLOW_LOOPBACK_DEV=true" \
    --env "WASI_AUTHZ_PDP_BEARER_TOKEN=${PDP_TOKEN}" \
    --dir="${ROOT}/tests/test-app/static::/static" \
    --addr "${APP_ADDRESS}" \
    "${COMPOSED}" \
    >"${TEMPORARY}/app.log" 2>&1 &
app_pid=$!

app_ready=false
for _ in $(seq 1 120); do
    if ! kill -0 "${app_pid}" 2>/dev/null; then
        cat "${TEMPORARY}/app.log" >&2 || true
        echo "error: composed lifecycle service exited before readiness" >&2
        exit 1
    fi
    status="$(curl --silent --max-time 1 \
        --header 'authorization: Bearer deny' \
        --output /dev/null \
        --write-out '%{http_code}' \
        "${APP_URL}/api/get_test" || true)"
    if [[ "${status}" == "503" ]]; then
        app_ready=true
        break
    fi
    sleep 0.1
done
[[ "${app_ready}" == "true" ]] || {
    echo "error: composed lifecycle service did not become ready" >&2
    exit 1
}

PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" allow \
    --base-url "${APP_URL}" --pdp-url "${PDP_URL}"
PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" faults \
    --base-url "${APP_URL}" --pdp-url "${PDP_URL}"
PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" disconnect \
    --base-url "${APP_URL}" --pdp-url "${PDP_URL}"
PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" saturation \
    --base-url "${APP_URL}" --pdp-url "${PDP_URL}"

kill "${pdp_pid}" >/dev/null 2>&1 || true
wait "${pdp_pid}" >/dev/null 2>&1 || true
pdp_pid=""
PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" outage \
    --base-url "${APP_URL}"
start_pdp
PYTHONDONTWRITEBYTECODE=1 python3 "${FIXTURE_ROOT}/exercise.py" recovery \
    --base-url "${APP_URL}" --pdp-url "${PDP_URL}"

sleep 1
expected_downstream=8
actual_downstream="$(grep -Fc 'authz-lifecycle-probe: downstream-invoked' "${TEMPORARY}/app.log" || true)"
[[ "${actual_downstream}" == "${expected_downstream}" ]] || {
    echo "error: downstream invocation count mismatch; expected ${expected_downstream}, found ${actual_downstream}" >&2
    cat "${TEMPORARY}/app.log" >&2
    exit 1
}

for sentinel in \
    'Bearer allow' \
    'Bearer deny' \
    "${PDP_TOKEN}" \
    'middleware-secret-cookie-sentinel' \
    'middleware-secret-query-sentinel' \
    'middleware-secret-identity-sentinel' \
    'middleware-secret-issuer-sentinel' \
    'user-1'; do
    for log in "${TEMPORARY}/app.log" "${TEMPORARY}/broker.log" "${TEMPORARY}/pdp.log"; do
        if grep -Fq -- "${sentinel}" "${log}"; then
            echo "error: lifecycle runtime log disclosed credential, cookie, query, or identity data" >&2
            echo "log: ${log}" >&2
            exit 1
        fi
    done
done

echo "authorization lifecycle E2E passed on Wasmtime $(root_lock_value wasmtime_version)"

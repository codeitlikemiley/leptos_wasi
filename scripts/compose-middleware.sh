#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/middleware-common.sh"

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <application.wasm> <output.wasm> <outer.wasm> [inner.wasm ...]" >&2
  exit 2
fi

APP="$1"
OUTPUT="$2"
shift 2
MIDDLEWARE=("$@")

WAC_BIN="$(resolve_middleware_tool WAC_BIN wac "$(middleware_lock_value wac_cli_version)")"
WASM_TOOLS_BIN="$(resolve_middleware_tool WASM_TOOLS_BIN wasm-tools "$(middleware_lock_value wasm_tools_version)")"

"$ROOT/scripts/audit-middleware-manifests.py" --composition-tools

if [[ ! -f "$APP" ]]; then
  echo "application component does not exist: $APP" >&2
  exit 2
fi

APP_WIT="$("$WASM_TOOLS_BIN" component wit "$APP")"
printf '%s\n' "$APP_WIT" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
  echo "application does not export final wasi:http/handler@0.3.0: $APP" >&2
  exit 1
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
CURRENT="$APP"

for ((index=${#MIDDLEWARE[@]} - 1; index >= 0; index--)); do
  COMPONENT="${MIDDLEWARE[$index]}"
  if [[ ! -f "$COMPONENT" ]]; then
    echo "middleware component does not exist: $COMPONENT" >&2
    exit 2
  fi

  WIT="$("$WASM_TOOLS_BIN" component wit "$COMPONENT")"
  printf '%s\n' "$WIT" | rg -q 'import wasi:http/handler@0\.3\.0;' || {
    echo "middleware does not import final downstream wasi:http/handler@0.3.0: $COMPONENT" >&2
    exit 1
  }
  printf '%s\n' "$WIT" | rg -q 'export wasi:http/handler@0\.3\.0;' || {
    echo "middleware does not export final wasi:http/handler@0.3.0: $COMPONENT" >&2
    exit 1
  }

  STAGE="$TMP/stage-$index.wasm"
  "$WAC_BIN" plug --plug "$CURRENT" "$COMPONENT" --output "$STAGE"
  CURRENT="$STAGE"
done

mkdir -p "$(dirname "$OUTPUT")"
install -m 0644 "$CURRENT" "$OUTPUT"
"$WASM_TOOLS_BIN" validate --features component-model,cm-async "$OUTPUT"
echo "composed middleware application: $OUTPUT"

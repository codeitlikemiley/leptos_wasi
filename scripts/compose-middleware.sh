#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <application.wasm> <output.wasm> <outer.wasm> [inner.wasm ...]" >&2
  exit 2
fi

APP="$1"
OUTPUT="$2"
shift 2
MIDDLEWARE=("$@")

for tool in wac wasm-tools; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "required command is not installed: $tool" >&2
    exit 2
  fi
done

"$ROOT/scripts/audit-middleware-manifests.py" --composition-tools

if [[ ! -f "$APP" ]]; then
  echo "application component does not exist: $APP" >&2
  exit 2
fi

APP_WIT="$(wasm-tools component wit "$APP")"
printf '%s\n' "$APP_WIT" | rg -q 'export wasi:http/handler@0\.3\.0-rc-2026-03-15' || {
  echo "application does not export the pinned WASIp3 HTTP handler world: $APP" >&2
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

  WIT="$(wasm-tools component wit "$COMPONENT")"
  printf '%s\n' "$WIT" | rg -q 'import wasi:http/handler@0\.3\.0-rc-2026-03-15' || {
    echo "middleware does not import the pinned downstream handler: $COMPONENT" >&2
    exit 1
  }
  printf '%s\n' "$WIT" | rg -q 'export wasi:http/handler@0\.3\.0-rc-2026-03-15' || {
    echo "middleware does not export the pinned HTTP handler: $COMPONENT" >&2
    exit 1
  }

  STAGE="$TMP/stage-$index.wasm"
  wac plug --plug "$CURRENT" "$COMPONENT" --output "$STAGE"
  CURRENT="$STAGE"
done

mkdir -p "$(dirname "$OUTPUT")"
install -m 0644 "$CURRENT" "$OUTPUT"
wasm-tools validate --features component-model,cm-async "$OUTPUT"
echo "composed middleware application: $OUTPUT"

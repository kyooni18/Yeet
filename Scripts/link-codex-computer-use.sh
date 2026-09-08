#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
LAUNCHER="$ROOT/Scripts/codex-computer-use-mcp.sh"
SERVER_NAME="${YEET_COMPUTER_USE_MCP_SERVER:-node_repl}"

if [ ! -f "$LAUNCHER" ]; then
  echo "Yeet Computer Use launcher was not found: $LAUNCHER" >&2
  exit 1
fi

if [ -n "${YEET_BIN:-}" ]; then
  if [ ! -x "$YEET_BIN" ]; then
    echo "YEET_BIN is not executable: $YEET_BIN" >&2
    exit 1
  fi
  "$YEET_BIN" mcp add-stdio "$SERVER_NAME" /bin/sh "$LAUNCHER"
elif command -v yeet >/dev/null 2>&1; then
  yeet mcp add-stdio "$SERVER_NAME" /bin/sh "$LAUNCHER"
elif command -v cargo >/dev/null 2>&1 && [ -f "$ROOT/Cargo.toml" ]; then
  (cd "$ROOT" && cargo run --quiet -- mcp add-stdio "$SERVER_NAME" /bin/sh "$LAUNCHER")
else
  echo "Yeet was not found. Install it, set YEET_BIN, or run this script from the Yeet checkout." >&2
  exit 1
fi

echo "Linked Codex unified Computer Use to Yeet as MCP server '$SERVER_NAME' (tools: js, js_reset)."

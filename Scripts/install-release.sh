#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PREFIX=${PREFIX:-"$HOME/.local"}
DEST="$PREFIX/bin"
RUNTIME_DEST="$PREFIX/share/yeet/runtime"
BINARY="$ROOT/bin/yeet"
RUNTIME="$ROOT/share/yeet/runtime"

if [ "${1:-}" = "--uninstall" ]; then
  if [ -x "$DEST/yeet" ]; then
    RUNNING_MCP_PORTS=$("$DEST/yeet" mcpserver list 2>/dev/null | awk '$2 == "running" { print $1 }' || true)
    for port in $RUNNING_MCP_PORTS; do
      "$DEST/yeet" mcpserver stop --port "$port" >/dev/null 2>&1 || true
    done
  fi
  rm -f "$DEST/yeet"
  rm -rf "$PREFIX/share/yeet"
  echo "Removed Yeet from $PREFIX"
  exit 0
fi

if [ ! -x "$BINARY" ] || [ ! -f "$RUNTIME/dist/bridge.js" ]; then
  echo "This installer must be run from an extracted Yeet release bundle." >&2
  exit 1
fi

RUNNING_MCP_PORTS=""
if [ -x "$DEST/yeet" ]; then
  RUNNING_MCP_PORTS=$("$DEST/yeet" mcpserver list 2>/dev/null | awk '$2 == "running" { print $1 }' || true)
  for port in $RUNNING_MCP_PORTS; do
    echo "Stopping Yeet MCP daemon on port $port before update"
    "$DEST/yeet" mcpserver stop --port "$port" >/dev/null
  done
fi

mkdir -p "$DEST" "$RUNTIME_DEST"
install -m 755 "$BINARY" "$DEST/yeet"
rm -rf "$RUNTIME_DEST/dist"
cp -R "$RUNTIME/dist" "$RUNTIME_DEST/dist"
cp "$RUNTIME/package.json" "$RUNTIME_DEST/package.json"

for port in $RUNNING_MCP_PORTS; do
  echo "Restarting Yeet MCP daemon on port $port"
  "$DEST/yeet" mcpserver start --port "$port" >/dev/null
 done

echo "Installed Yeet to $DEST/yeet"
echo "Installed runtime to $RUNTIME_DEST"
case ":${PATH:-}:" in
  *":$DEST:"*) ;;
  *) echo "Add $DEST to PATH to run yeet by name." ;;
esac

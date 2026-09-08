#!/bin/sh
set -eu

CODEX_HOME=${CODEX_HOME:-"$HOME/.codex"}
if [ -n "${CODEX_APP:-}" ]; then
  :
elif [ -d "/Applications/ChatGPT.app" ]; then
  CODEX_APP="/Applications/ChatGPT.app"
elif [ -d "/Applications/Codex.app" ]; then
  CODEX_APP="/Applications/Codex.app"
else
  CODEX_APP="/Applications/ChatGPT.app"
fi
RESOURCES="$CODEX_APP/Contents/Resources"
CUA_NODE="$RESOURCES/cua_node/bin/node"
CUA_REPL="$RESOURCES/cua_node/bin/node_repl"
CUA_MODULES="$RESOURCES/cua_node/lib/node_modules"
CODEX_CLI="$RESOURCES/codex"
SKY_APP="$CODEX_HOME/computer-use/Codex Computer Use.app"
PLUGIN_ROOT="$CODEX_HOME/plugins/cache/openai-bundled/unified-computer-use"

if [ ! -x "$CUA_NODE" ] || [ ! -x "$CUA_REPL" ]; then
  echo "Codex unified Computer Use runtime was not found in $CODEX_APP" >&2
  exit 1
fi
if [ ! -d "$SKY_APP" ]; then
  echo "Codex Computer Use service was not found at $SKY_APP" >&2
  exit 1
fi
if [ ! -d "$PLUGIN_ROOT" ]; then
  echo "Codex unified-computer-use plugin is not installed under $PLUGIN_ROOT" >&2
  exit 1
fi

# macOS ships BSD sort, which has no GNU -V flag. Version folders use dotted
# numeric names, so sort -t/-k gives us a portable newest-version selection.
PLUGIN=$(find "$PLUGIN_ROOT" -mindepth 1 -maxdepth 1 -type d -print | sort | tail -n 1)
LAUNCHER="$PLUGIN/scripts/launch.mjs"
if [ ! -f "$LAUNCHER" ]; then
  echo "Codex unified Computer Use launcher was not found at $LAUNCHER" >&2
  exit 1
fi

export NODE_REPL_NATIVE_PIPE_CONNECT_TIMEOUT_MS=${NODE_REPL_NATIVE_PIPE_CONNECT_TIMEOUT_MS:-1000}
export NODE_REPL_NODE_MODULE_DIRS="$CUA_MODULES"
export NODE_REPL_NODE_PATH="$CUA_NODE"
export NODE_REPL_TRUSTED_CODE_PATHS="$CODEX_HOME:$CUA_MODULES"
export CODEX_HOME
export SKY_CUA_SERVICE_PATH="$SKY_APP"
export CODEX_CLI_PATH="$CODEX_CLI"
export CUA_REPL_NODE_REPL_PATH="$CUA_REPL"
export CUA_REPL_ENABLED_SURFACES=${CUA_REPL_ENABLED_SURFACES:-computer}

exec "$CUA_NODE" "$LAUNCHER"

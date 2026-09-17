#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
RUNTIME_SOURCE_DIR="$ROOT_DIR/RuntimeSource"
WEB_DIR="$ROOT_DIR/web"
WEB_DEPS_STAMP="$ROOT_DIR/target/serve-web-deps.stamp"

readonly ROOT_DIR RUNTIME_SOURCE_DIR WEB_DIR WEB_DEPS_STAMP

log() {
  printf '==> %s\n' "$*"
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  local command_name="$1"
  local description="$2"

  command -v "$command_name" >/dev/null 2>&1 || die "$description"
}

check_node() {
  local node_major

  require_command node 'Node.js 20 or newer is required for the Yeet runtime.'
  node_major="$(node -p 'process.versions.node.split(".")[0]')" || die 'Unable to determine the Node.js version.'

  if [[ ! "$node_major" =~ ^[0-9]+$ ]] || (( node_major < 20 )); then
    die "Yeet requires Node.js 20 or newer; found $(node --version 2>/dev/null || printf 'an unknown version')."
  fi
}

sources_changed_since() {
  local stamp="$1"
  shift

  [[ -f "$stamp" ]] || return 0

  local input
  for input in "$@"; do
    [[ -e "$input" ]] || continue

    if [[ -f "$input" && "$input" -nt "$stamp" ]]; then
      return 0
    fi

    if [[ -d "$input" ]] && [[ -n "$(find "$input" -type f -newer "$stamp" -print -quit)" ]]; then
      return 0
    fi
  done

  return 1
}

ensure_runtime_dependencies() {
  local tsc="$RUNTIME_SOURCE_DIR/node_modules/.bin/tsc"
  local installed_lock="$RUNTIME_SOURCE_DIR/node_modules/.package-lock.json"

  if [[ -x "$tsc" && -f "$installed_lock" && ! "$RUNTIME_SOURCE_DIR/package-lock.json" -nt "$installed_lock" && ! "$RUNTIME_SOURCE_DIR/package.json" -nt "$installed_lock" ]]; then
    return
  fi

  require_command npm 'npm is required to install Yeet runtime build dependencies.'
  log 'Synchronizing Yeet runtime dependencies'
  npm --prefix "$RUNTIME_SOURCE_DIR" ci
}

ensure_web_dependencies() {
  if [[ -d "$WEB_DIR/node_modules" && -f "$WEB_DEPS_STAMP" ]] && ! sources_changed_since "$WEB_DEPS_STAMP" \
    "$WEB_DIR/package.json" \
    "$WEB_DIR/pnpm-lock.yaml"; then
    return
  fi

  log 'Synchronizing Yeet Remote WebUI dependencies'
  if command -v pnpm >/dev/null 2>&1; then
    pnpm --dir "$WEB_DIR" install --frozen-lockfile
  elif command -v corepack >/dev/null 2>&1; then
    corepack pnpm --dir "$WEB_DIR" install --frozen-lockfile
  else
    die 'pnpm or Corepack is required to install the Yeet Remote WebUI dependencies.'
  fi

  touch "$WEB_DEPS_STAMP"
}

build_webui() {
  # Always rebuild this output. Cargo embeds web/dist into the native release
  # binary, so a stale dist directory would otherwise produce a stale server.
  log 'Rebuilding Yeet Remote WebUI'
  "$ROOT_DIR/Scripts/build-remote-web.sh"
  [[ -f "$WEB_DIR/dist/index.html" ]] || die 'Remote WebUI build did not produce web/dist/index.html.'
}

replace_local_install() {
  log 'Rebuilding Yeet TypeScript runtime and native TUI, then replacing the installed binary'
  "$ROOT_DIR/Scripts/install-local.sh"

  [[ -x "$ROOT_DIR/target/release/yeet" ]] || die 'Rust build did not produce target/release/yeet.'
  [[ -f "$ROOT_DIR/target/install-runtime/dist/bridge.js" ]] || die 'Runtime build did not produce target/install-runtime/dist/bridge.js.'
}

installed_yeet_path() {
  if [[ -n "${PREFIX+x}" ]]; then
    printf '%s/bin/yeet\n' "$PREFIX"
  elif command -v brew >/dev/null 2>&1; then
    printf '%s/bin/yeet\n' "$(brew --prefix)"
  elif command -v yeet >/dev/null 2>&1; then
    command -v yeet
  else
    # install-local.sh falls back to this prefix when no writable PATH prefix
    # is available. The release artifact is a safe final fallback for starting
    # the just-built server even if that prefix is not on PATH.
    printf '%s/target/release/yeet\n' "$ROOT_DIR"
  fi
}

main() {
  cd "$ROOT_DIR"

  require_command cargo 'Rust/Cargo is required to build Yeet.'
  check_node
  ensure_runtime_dependencies
  ensure_web_dependencies
  build_webui
  replace_local_install

  export YEET_RUNTIME_DIR="$ROOT_DIR/target/install-runtime"

  log 'Starting Yeet Remote'
  YEET_BIN="$(installed_yeet_path)"
  [[ -x "$YEET_BIN" ]] || die "Replaced Yeet binary is not executable: $YEET_BIN"
  exec "$YEET_BIN" remote "$@"
}

main "$@"

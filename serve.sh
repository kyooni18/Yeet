#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'error: Rust/Cargo is required to build Yeet.' >&2
  exit 1
fi

if ! command -v node >/dev/null 2>&1; then
  printf '%s\n' 'error: Node.js is required for the Yeet runtime.' >&2
  exit 1
fi

NODE_MAJOR="$(node -p 'Number(process.versions.node.split(".")[0])')"
if [[ ! "$NODE_MAJOR" =~ ^[0-9]+$ ]] || (( NODE_MAJOR < 20 )); then
  printf '%s\n' 'error: Yeet requires Node.js 20 or newer.' >&2
  exit 1
fi

if [[ ! -x "$ROOT_DIR/RuntimeSource/node_modules/.bin/tsc" ]]; then
  if ! command -v npm >/dev/null 2>&1; then
    printf '%s\n' 'error: npm is required to install Yeet runtime dependencies.' >&2
    exit 1
  fi

  printf '%s\n' 'Installing Yeet runtime dependencies...'
  (
    cd "$ROOT_DIR/RuntimeSource"
    npm ci
  )
fi

printf '%s\n' 'Building and installing Yeet...'
"$ROOT_DIR/Scripts/install-local.sh"

export YEET_RUNTIME_DIR="$ROOT_DIR/target/install-runtime"

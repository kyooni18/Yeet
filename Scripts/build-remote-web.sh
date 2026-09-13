#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WEB="$ROOT/web"
DIST=${YEET_REMOTE_WEB_DIST:-"$WEB/dist"}

if [ ! -f "$WEB/package.json" ]; then
  echo "Yeet Remote WebUI source is missing: $WEB/package.json" >&2
  exit 1
fi

if command -v pnpm >/dev/null 2>&1; then
  if [ ! -d "$WEB/node_modules" ]; then
    pnpm --dir "$WEB" install --frozen-lockfile
  fi
  pnpm --dir "$WEB" build
elif command -v corepack >/dev/null 2>&1; then
  if [ ! -d "$WEB/node_modules" ]; then
    corepack pnpm --dir "$WEB" install --frozen-lockfile
  fi
  corepack pnpm --dir "$WEB" build
elif command -v npm >/dev/null 2>&1; then
  if [ ! -d "$WEB/node_modules" ]; then
    echo "pnpm is required to install the locked Remote WebUI dependencies." >&2
    exit 1
  fi
  npm --prefix "$WEB" run build
else
  echo "Node package runner not found. Install pnpm (preferred) or npm." >&2
  exit 1
fi

if [ ! -f "$WEB/dist/index.html" ]; then
  echo "Remote WebUI build did not produce web/dist/index.html" >&2
  exit 1
fi

if [ "$DIST" != "$WEB/dist" ]; then
  rm -rf "$DIST"
  mkdir -p "$(dirname -- "$DIST")"
  cp -R "$WEB/dist" "$DIST"
fi

if grep -q '/src/main\.ts' "$DIST/index.html"; then
  echo "Remote WebUI output still references Vite development source." >&2
  exit 1
fi

echo "Remote WebUI rebuilt at $DIST"

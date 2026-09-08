#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SRC="$ROOT/RuntimeSource"
OUT=${YEET_RUNTIME_OUT_DIR:-"$SRC/dist"}

case "$OUT" in
  /*) ;;
  *) OUT="$ROOT/$OUT" ;;
esac

cd "$SRC"
if [ -x "node_modules/.bin/tsc" ]; then
  TSC="node_modules/.bin/tsc"
elif command -v tsc >/dev/null 2>&1; then
  TSC=$(command -v tsc)
else
  echo "TypeScript compiler not found. Run npm install in RuntimeSource first." >&2
  exit 1
fi

if [ -e "$OUT" ]; then
  rm -rf "$OUT"
fi
mkdir -p "$OUT"

"$TSC" -p tsconfig.json --outDir "$OUT"
node --check "$OUT/bridge.js"
node --check "$OUT/edit-backend/daemon.js"
echo "Runtime rebuilt at $OUT"

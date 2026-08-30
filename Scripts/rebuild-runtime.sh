#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SRC="$ROOT/RuntimeSource"
DST="$ROOT/Sources/HarnessCallCore/Resources/Runtime"

cd "$SRC"
if [ -x "node_modules/.bin/tsc" ]; then
  TSC="node_modules/.bin/tsc"
elif command -v tsc >/dev/null 2>&1; then
  TSC=$(command -v tsc)
else
  echo "TypeScript compiler not found. Run npm install in RuntimeSource first." >&2
  exit 1
fi

rm -rf dist
"$TSC" -p tsconfig.json

rm -rf "$DST/dist"
mkdir -p "$DST/dist"
cp -R dist/. "$DST/dist/"
printf '%s\n' '{"type":"module","name":"yeet-call-core-runtime","version":"0.1.0","private":true}' > "$DST/package.json"

node --check "$DST/dist/bridge.js"
echo "Bundled runtime rebuilt at $DST"

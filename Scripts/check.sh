#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

./Scripts/rebuild-runtime.sh
(
  cd RuntimeSource
  node --test test/*.test.mjs
)
cargo test

TMP_YEET=$(mktemp -d)
trap 'rm -rf "$TMP_YEET"' EXIT INT TERM
YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- doctor >/dev/null
YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- model set openai/test-model >/dev/null
[ "$(YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- model get)" = "openai/test-model" ]

echo "All checks passed."

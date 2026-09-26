#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

npm --prefix RuntimeSource ci
npm --prefix web ci
npm --prefix RuntimeSource run check
npm --prefix web run typecheck
cargo test

TMP_YEET=$(mktemp -d)
trap 'rm -rf "$TMP_YEET"' EXIT INT TERM
YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- doctor >/dev/null
YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- model set openai/test-model >/dev/null
[ "$(YEET_CONFIG_DIR="$TMP_YEET" cargo run --quiet -- model get)" = "openai/test-model" ]

echo "All checks passed."

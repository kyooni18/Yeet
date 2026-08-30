#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

./Scripts/rebuild-runtime.sh
(
  cd RuntimeSource
  node --test test/*.test.mjs
)
swift test

TMP_YEET=$(mktemp -d)
trap 'rm -rf "$TMP_YEET"' EXIT INT TERM
YEET_CONFIG_DIR="$TMP_YEET" swift run yeet doctor
YEET_CONFIG_DIR="$TMP_YEET" swift run yeet model set openai/test-model
[ "$(YEET_CONFIG_DIR="$TMP_YEET" swift run yeet model get)" = "openai/test-model" ]

echo "All checks passed."

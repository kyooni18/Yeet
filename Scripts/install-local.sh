#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=${PREFIX:-"$HOME/.local"}
DEST="$PREFIX/bin"

cd "$ROOT"
swift build -c release
BIN_DIR=$(swift build -c release --show-bin-path)
mkdir -p "$DEST"
cp "$BIN_DIR/yeet" "$DEST/yeet"
chmod +x "$DEST/yeet"

for bundle in "$BIN_DIR"/Yeet_HarnessCallCore.*; do
  [ -d "$bundle" ] || continue
  name=$(basename "$bundle")
  rm -rf "$DEST/$name"
  cp -R "$bundle" "$DEST/$name"
done

echo "Installed yeet to $DEST/yeet"

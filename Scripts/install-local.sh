#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

path_prefix() {
  old_ifs=$IFS
  IFS=:
  for entry in ${PATH:-}; do
    [ -n "$entry" ] || entry=.
    case "$entry" in
      */bin)
        candidate=${entry%/bin}
        if [ -n "$candidate" ] && [ -d "$candidate" ] && [ -w "$candidate" ]; then
          IFS=$old_ifs
          printf '%s\n' "$candidate"
          return 0
        fi
        ;;
    esac
  done
  IFS=$old_ifs
  return 1
}

if [ "${PREFIX+x}" = x ]; then
  if [ -z "$PREFIX" ]; then
    echo "PREFIX must not be empty." >&2
    exit 1
  fi
elif command -v brew >/dev/null 2>&1; then
  PREFIX=$(brew --prefix)
elif PREFIX=$(path_prefix); then
  :
else
  PREFIX="$HOME/.local"
fi

DEST="$PREFIX/bin"
RUNTIME_DEST="$PREFIX/share/yeet/runtime"
RUNTIME_BUILD="$ROOT/target/install-runtime"

cd "$ROOT"
YEET_RUNTIME_OUT_DIR="$RUNTIME_BUILD/dist" "$ROOT/Scripts/rebuild-runtime.sh"
cargo build --release
mkdir -p "$DEST" "$RUNTIME_DEST"
install -m 755 target/release/yeet "$DEST/yeet"
if ! cmp -s target/release/yeet "$DEST/yeet"; then
  echo "Installed yeet binary does not match the freshly built Rust release." >&2
  exit 1
fi
rm -rf "$RUNTIME_DEST/dist"
cp -R "$RUNTIME_BUILD/dist" "$RUNTIME_DEST/dist"
cp RuntimeSource/package.json "$RUNTIME_DEST/package.json"

echo "Install prefix: $PREFIX"
echo "Installed yeet to $DEST/yeet"
echo "Installed runtime to $RUNTIME_DEST"

case ":${PATH:-}:" in
  *":$DEST:"*) ;;
  *) echo "Note: add $DEST to PATH to run yeet by name." ;;
esac

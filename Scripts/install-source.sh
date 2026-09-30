#!/bin/sh
set -eu

REPO=${YEET_REPO:-https://github.com/kyooni18/Yeet.git}
SOURCE=${YEET_SOURCE_DIR:-"$HOME/.local/src/yeet"}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Yeet build requires: $1" >&2
    exit 1
  }
}

if [ -f "Cargo.toml" ] && [ -d "RuntimeSource" ] && [ -d "web" ]; then
  ROOT=$(pwd)
else
  need git
  if [ -d "$SOURCE/.git" ]; then
    git -C "$SOURCE" pull --ff-only
  else
    rm -rf "$SOURCE"
    mkdir -p "$(dirname "$SOURCE")"
    git clone --depth 1 "$REPO" "$SOURCE"
  fi
  ROOT=$SOURCE
fi

# Use the same full replacement lifecycle as release installation.
exec sh "$ROOT/install.sh" --build "$@"

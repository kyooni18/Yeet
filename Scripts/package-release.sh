#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${YEET_PACKAGE_OUT:-"$ROOT/target/packages"}
VERSION=${YEET_VERSION:-$(sed -n '/^\[package\]/,/^\[/{s/^version = "\([^"]*\)"/\1/p;}' "$ROOT/Cargo.toml" | head -n 1)}
VERSION=${VERSION#v}
TARGET=$(rustc -vV | sed -n 's/^host: //p')
NAME="yeet-$VERSION-$TARGET"
STAGE_BASE="$ROOT/target/package-stage"
STAGE="$STAGE_BASE/$NAME"
RUNTIME_BUILD="$ROOT/target/release-runtime-$TARGET"

checksum() {
  file=$1
  base=$(basename -- "$file")
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUT" && sha256sum "$base" > "$base.sha256")
  else
    (cd "$OUT" && shasum -a 256 "$base" > "$base.sha256")
  fi
}

cd "$ROOT"
if [ ! -x RuntimeSource/node_modules/.bin/tsc ]; then
  npm --prefix RuntimeSource ci
fi
YEET_RUNTIME_OUT_DIR="$RUNTIME_BUILD/dist" "$ROOT/Scripts/rebuild-runtime.sh"
"$ROOT/Scripts/build-remote-web.sh"
cargo build --release

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/yeet/runtime" "$OUT"
install -m 755 target/release/yeet "$STAGE/bin/yeet"
cp -R "$RUNTIME_BUILD/dist" "$STAGE/share/yeet/runtime/dist"
cp RuntimeSource/package.json "$STAGE/share/yeet/runtime/package.json"
cp README.md LICENSE.txt "$STAGE/"
cp Scripts/install-release.sh "$STAGE/install.sh"
chmod 755 "$STAGE/install.sh"
if [ -f docs/PLATFORM_SUPPORT.md ]; then
  mkdir -p "$STAGE/docs"
  cp docs/PLATFORM_SUPPORT.md "$STAGE/docs/PLATFORM_SUPPORT.md"
fi

ARCHIVE="$OUT/$NAME.tar.gz"
rm -f "$ARCHIVE" "$ARCHIVE.sha256"
tar -C "$STAGE_BASE" -czf "$ARCHIVE" "$NAME"
checksum "$ARCHIVE"

case "$TARGET" in
  *linux*)
    if command -v dpkg-deb >/dev/null 2>&1; then
      case "$TARGET" in
        x86_64-*) DEB_ARCH=amd64 ;;
        aarch64-*) DEB_ARCH=arm64 ;;
        *) DEB_ARCH=all ;;
      esac
      DEB_ROOT="$ROOT/target/deb-stage/$NAME"
      rm -rf "$DEB_ROOT"
      mkdir -p "$DEB_ROOT/DEBIAN" "$DEB_ROOT/usr/bin" "$DEB_ROOT/usr/share/yeet/runtime" "$DEB_ROOT/usr/share/doc/yeet"
      install -m 755 target/release/yeet "$DEB_ROOT/usr/bin/yeet"
      cp -R "$RUNTIME_BUILD/dist" "$DEB_ROOT/usr/share/yeet/runtime/dist"
      cp RuntimeSource/package.json "$DEB_ROOT/usr/share/yeet/runtime/package.json"
      cp README.md LICENSE.txt "$DEB_ROOT/usr/share/doc/yeet/"
      cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: yeet
Version: $VERSION
Section: utils
Priority: optional
Architecture: $DEB_ARCH
Depends: nodejs (>= 20)
Maintainer: Yeet contributors
Description: Native multiplatform agent CLI and remote UI
 Yeet is a Rust CLI/TUI with a TypeScript provider and tool runtime.
EOF
      DEB="$OUT/yeet_${VERSION}_${DEB_ARCH}.deb"
      rm -f "$DEB" "$DEB.sha256"
      dpkg-deb --root-owner-group --build "$DEB_ROOT" "$DEB"
      checksum "$DEB"
    fi
    ;;
esac

printf 'Created release package(s) in %s\n' "$OUT"

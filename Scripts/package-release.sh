#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${YEET_PACKAGE_OUT:-"$ROOT/target/packages"}
VERSION=${YEET_VERSION:-$(sed -n '/^\[package\]/,/^\[/{s/^version = "\([^"]*\)"/\1/p;}' "$ROOT/Cargo.toml" | head -n 1)}
VERSION=${VERSION#v}
node "$ROOT/Scripts/verify-release-version.mjs" "$VERSION"
TARGET=$(rustc -vV | sed -n 's/^host: //p')
NAME="yeet-$VERSION-$TARGET"
STAGE_BASE="$ROOT/target/package-stage"
STAGE="$STAGE_BASE/$NAME"

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
npm --prefix RuntimeSource ci
npm --prefix web ci
npm --prefix RuntimeSource run build
npm --prefix web run build
cargo build --release

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/yeet/runtime" "$OUT"
install -m 755 target/release/yeet "$STAGE/bin/yeet"
cp -R RuntimeSource/dist "$STAGE/share/yeet/runtime/dist"
cp -R RuntimeSource/skills "$STAGE/share/yeet/runtime/skills"
cp RuntimeSource/package.json "$STAGE/share/yeet/runtime/package.json"
cp README.md CHANGELOG.md LICENSE.txt "$STAGE/"
[ -f "$STAGE/share/yeet/runtime/skills/pdf/SKILL.md" ] || {
  echo "Staged release is missing bundled PDF skill" >&2
  exit 1
}
if [ -f docs/PLATFORM_SUPPORT.md ]; then
  mkdir -p "$STAGE/docs"
  cp docs/PLATFORM_SUPPORT.md "$STAGE/docs/PLATFORM_SUPPORT.md"
fi

SMOKE_CONFIG="$ROOT/target/release-smoke-config-$TARGET"
rm -rf "$SMOKE_CONFIG"
ACTUAL_VERSION=$("$STAGE/bin/yeet" --version)
[ "$ACTUAL_VERSION" = "$VERSION" ] || {
  echo "Staged Yeet version $ACTUAL_VERSION does not match release version $VERSION" >&2
  exit 1
}
YEET_CONFIG_DIR="$SMOKE_CONFIG" "$STAGE/bin/yeet" doctor >/dev/null
rm -rf "$SMOKE_CONFIG"

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
      cp -R RuntimeSource/dist "$DEB_ROOT/usr/share/yeet/runtime/dist"
      cp -R RuntimeSource/skills "$DEB_ROOT/usr/share/yeet/runtime/skills"
      cp RuntimeSource/package.json "$DEB_ROOT/usr/share/yeet/runtime/package.json"
      cp README.md CHANGELOG.md LICENSE.txt "$DEB_ROOT/usr/share/doc/yeet/"
      cat > "$DEB_ROOT/DEBIAN/control" <<CONTROL
Package: yeet
Version: $VERSION
Section: utils
Priority: optional
Architecture: $DEB_ARCH
Depends: nodejs
Maintainer: Yeet contributors
Description: Native multiplatform agent CLI and remote UI
 Yeet is a Rust CLI/TUI with a TypeScript provider and tool runtime.
CONTROL
      DEB="$OUT/yeet_${VERSION}_${DEB_ARCH}.deb"
      rm -f "$DEB" "$DEB.sha256"
      dpkg-deb --root-owner-group --build "$DEB_ROOT" "$DEB"
      checksum "$DEB"
    fi
    ;;
esac

printf 'Created release package(s) in %s\n' "$OUT"

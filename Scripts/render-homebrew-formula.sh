#!/bin/sh
set -eu

VERSION=${1:-}
ASSETS=${2:-}
OUTPUT=${3:-}

[ -n "$VERSION" ] || {
  echo "Usage: $0 <version|tag> <release-assets-dir> <output-formula>" >&2
  exit 2
}
[ -n "$ASSETS" ] || {
  echo "release assets directory is required" >&2
  exit 2
}
[ -n "$OUTPUT" ] || {
  echo "output formula path is required" >&2
  exit 2
}

VERSION=${VERSION#v}
TAG="v$VERSION"

checksum() {
  name=$1
  file="$ASSETS/$name.sha256"
  [ -f "$file" ] || {
    echo "Missing checksum: $file" >&2
    exit 1
  }
  value=$(awk '{print $1; exit}' "$file")
  case "$value" in
    ''|*[!0-9a-fA-F]*)
      echo "Invalid SHA-256 checksum in $file" >&2
      exit 1
      ;;
  esac
  [ "${#value}" -eq 64 ] || {
    echo "Invalid SHA-256 checksum length in $file" >&2
    exit 1
  }
  printf '%s' "$value"
}

MAC_ARM="yeet-$VERSION-aarch64-apple-darwin.tar.gz"
MAC_X64="yeet-$VERSION-x86_64-apple-darwin.tar.gz"
LINUX_ARM="yeet-$VERSION-aarch64-unknown-linux-gnu.tar.gz"
LINUX_X64="yeet-$VERSION-x86_64-unknown-linux-gnu.tar.gz"

MAC_ARM_SHA=$(checksum "$MAC_ARM")
MAC_X64_SHA=$(checksum "$MAC_X64")
LINUX_ARM_SHA=$(checksum "$LINUX_ARM")
LINUX_X64_SHA=$(checksum "$LINUX_X64")

mkdir -p "$(dirname "$OUTPUT")"
cat > "$OUTPUT" <<FORMULA
class Yeet < Formula
  desc "Native multiplatform agent CLI/TUI with a provider-neutral runtime and remote UI"
  homepage "https://github.com/kyooni18/Yeet"
  version "$VERSION"
  license "Apache-2.0"

  depends_on "node"

  on_macos do
    on_arm do
      url "https://github.com/kyooni18/Yeet/releases/download/$TAG/$MAC_ARM"
      sha256 "$MAC_ARM_SHA"
    end
    on_intel do
      url "https://github.com/kyooni18/Yeet/releases/download/$TAG/$MAC_X64"
      sha256 "$MAC_X64_SHA"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/kyooni18/Yeet/releases/download/$TAG/$LINUX_ARM"
      sha256 "$LINUX_ARM_SHA"
    end
    on_intel do
      url "https://github.com/kyooni18/Yeet/releases/download/$TAG/$LINUX_X64"
      sha256 "$LINUX_X64_SHA"
    end
  end

  def install
    bin.install "bin/yeet"
    (share/"yeet").install "share/yeet/runtime"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/yeet --version")
  end
end
FORMULA

printf 'Rendered %s\n' "$OUTPUT"

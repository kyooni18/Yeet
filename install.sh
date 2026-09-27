#!/bin/sh
set -eu

REPOSITORY=${YEET_REPOSITORY:-kyooni18/Yeet}
PREFIX=${YEET_PREFIX:-${PREFIX:-"$HOME/.local"}}

die() {
  echo "Yeet installer: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

check_node() {
  need node
  major=$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || printf '0')
  case "$major" in
    ''|*[!0-9]*) die "could not determine Node.js version" ;;
  esac
  [ "$major" -ge 20 ] || die "Node.js 20 or newer is required (found $(node --version 2>/dev/null || printf unknown))"
}

sha256_file() {
  file=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    die "sha256sum or shasum is required to verify the release"
  fi
}

install_bundle() {
  bundle=$1
  [ -x "$bundle/bin/yeet" ] || die "release bundle is missing bin/yeet"
  [ -f "$bundle/share/yeet/runtime/dist/bridge.js" ] || die "release bundle is missing the Yeet runtime"
  check_node
  need install

  mkdir -p "$PREFIX/bin" "$PREFIX/share/yeet" "$PREFIX/share/doc/yeet"

  bin_tmp="$PREFIX/bin/.yeet-install-$$"
  rm -f "$bin_tmp"
  install -m 755 "$bundle/bin/yeet" "$bin_tmp"
  mv -f "$bin_tmp" "$PREFIX/bin/yeet"

  runtime_root="$PREFIX/share/yeet"
  runtime_new="$runtime_root/.runtime-new-$$"
  runtime_old="$runtime_root/.runtime-old-$$"
  rm -rf "$runtime_new" "$runtime_old"
  cp -R "$bundle/share/yeet/runtime" "$runtime_new"

  if [ -e "$runtime_root/runtime" ]; then
    mv "$runtime_root/runtime" "$runtime_old"
  fi
  if mv "$runtime_new" "$runtime_root/runtime"; then
    rm -rf "$runtime_old"
  else
    [ ! -e "$runtime_old" ] || mv "$runtime_old" "$runtime_root/runtime"
    die "failed to install the Yeet runtime"
  fi

  for file in README.md CHANGELOG.md LICENSE.txt; do
    [ ! -f "$bundle/$file" ] || cp "$bundle/$file" "$PREFIX/share/doc/yeet/$file"
  done

  version=$("$PREFIX/bin/yeet" --version 2>/dev/null || printf unknown)
  printf 'Installed Yeet %s to %s/bin/yeet\n' "$version" "$PREFIX"
  case ":${PATH:-}:" in
    *":$PREFIX/bin:"*) ;;
    *) printf 'Add %s/bin to PATH.\n' "$PREFIX" ;;
  esac
}

bundle_dir() {
  script=$0
  case "$script" in
    /*) ;;
    */*) script="$(pwd)/$script" ;;
    *) script=$(command -v "$script" 2>/dev/null || true) ;;
  esac
  [ -n "$script" ] || return 1
  [ -f "$script" ] || return 1
  dir=$(CDPATH= cd -- "$(dirname -- "$script")" 2>/dev/null && pwd) || return 1
  [ -x "$dir/bin/yeet" ] || return 1
  [ -f "$dir/share/yeet/runtime/dist/bridge.js" ] || return 1
  printf '%s\n' "$dir"
}

if bundle=$(bundle_dir); then
  install_bundle "$bundle"
  exit 0
fi

need curl
need tar
check_node

curl_get() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL -A "Yeet installer" -H "Accept: application/vnd.github+json" -H "Authorization: Bearer $GITHUB_TOKEN" "$@"
  else
    curl -fsSL -A "Yeet installer" -H "Accept: application/vnd.github+json" "$@"
  fi
}

tag=${YEET_VERSION:-}
if [ -z "$tag" ]; then
  metadata=$(curl_get "https://api.github.com/repos/$REPOSITORY/releases/latest")
  tag=$(printf '%s\n' "$metadata" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "$tag" ] || die "could not determine the latest GitHub release"
fi

case "$tag" in
  v*) version=${tag#v} ;;
  *) version=$tag; tag="v$tag" ;;
esac

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-gnu ;;
  *) die "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) die "unsupported CPU architecture: $(uname -m)" ;;
esac

target="$arch-$os"
base="yeet-$version-$target"
archive="$base.tar.gz"
url="https://github.com/$REPOSITORY/releases/download/$tag/$archive"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/yeet-install.XXXXXX")
trap 'rm -rf "$tmp"' 0 HUP INT TERM

printf 'Downloading Yeet %s for %s...\n' "$version" "$target"
curl_get -o "$tmp/$archive" "$url"
curl_get -o "$tmp/$archive.sha256" "$url.sha256"

expected=$(awk '{print $1; exit}' "$tmp/$archive.sha256")
actual=$(sha256_file "$tmp/$archive")
[ -n "$expected" ] || die "release checksum is empty"
[ "$actual" = "$expected" ] || die "release SHA-256 verification failed"

tar -xzf "$tmp/$archive" -C "$tmp"
[ -d "$tmp/$base" ] || die "release archive did not contain $base"

PREFIX="$PREFIX" sh "$tmp/$base/install.sh"

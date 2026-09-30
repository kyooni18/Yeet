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

configure_linux_path_file() {
  file=$1
  bin_dir=$2

  mkdir -p "$(dirname -- "$file")"
  [ -f "$file" ] || : > "$file"
  grep -F "$bin_dir" "$file" >/dev/null 2>&1 && return 0
  printf '\n# Added by Yeet installer\nexport PATH="%s:$PATH"\n' "$bin_dir" >> "$file"
}

configure_linux_path() {
  [ "$(uname -s)" = Linux ] || return 0
  [ "${YEET_NO_PATH_UPDATE:-0}" != 1 ] || return 0

  bin_dir="$PREFIX/bin"
  case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) PATH="$bin_dir:${PATH:-}"; export PATH ;;
  esac

  shell_name=$(basename -- "${SHELL:-sh}")
  case "$shell_name" in
    bash)
      configure_linux_path_file "$HOME/.profile" "$bin_dir"
      configure_linux_path_file "$HOME/.bashrc" "$bin_dir"
      printf 'Configured Linux PATH in %s and %s\n' "$HOME/.profile" "$HOME/.bashrc"
      ;;
    zsh)
      zdotdir=${ZDOTDIR:-"$HOME"}
      configure_linux_path_file "$HOME/.profile" "$bin_dir"
      configure_linux_path_file "$zdotdir/.zshrc" "$bin_dir"
      printf 'Configured Linux PATH in %s and %s\n' "$HOME/.profile" "$zdotdir/.zshrc"
      ;;
    fish)
      fish_config="$HOME/.config/fish/conf.d/yeet.fish"
      mkdir -p "$(dirname -- "$fish_config")"
      printf 'fish_add_path "%s"\n' "$bin_dir" > "$fish_config"
      configure_linux_path_file "$HOME/.profile" "$bin_dir"
      printf 'Configured Linux PATH in %s and %s\n' "$HOME/.profile" "$fish_config"
      ;;
    *)
      configure_linux_path_file "$HOME/.profile" "$bin_dir"
      printf 'Configured Linux PATH in %s\n' "$HOME/.profile"
      ;;
  esac
}

install_artifacts() {
  binary=$1
  runtime=$2
  docs=$3

  [ -x "$binary" ] || die "install source is missing the Yeet binary"
  [ -f "$runtime/dist/bridge.js" ] || die "install source is missing the Yeet runtime"
  [ -d "$runtime/skills" ] || die "install source is missing runtime skills"
  [ -f "$runtime/package.json" ] || die "install source is missing runtime package metadata"
  check_node
  need install

  mkdir -p "$PREFIX/bin" "$PREFIX/share/yeet" "$PREFIX/share/doc/yeet"

  bin_tmp="$PREFIX/bin/.yeet-install-$$"
  rm -f "$bin_tmp"
  install -m 755 "$binary" "$bin_tmp"

  runtime_root="$PREFIX/share/yeet"
  runtime_new="$runtime_root/.runtime-new-$$"
  runtime_old="$runtime_root/.runtime-old-$$"
  rm -rf "$runtime_new" "$runtime_old"
  mkdir -p "$runtime_new"
  cp -R "$runtime/dist" "$runtime_new/dist"
  cp -R "$runtime/skills" "$runtime_new/skills"
  cp "$runtime/package.json" "$runtime_new/package.json"

  lifecycle="$docs/Scripts/install-lifecycle.mjs"
  [ -f "$lifecycle" ] || die "install source is missing process lifecycle helper"
  if [ "$(uname -s)" = Darwin ]; then
    need python3
    need lsof
  fi
  state_file="$runtime_root/.restart-$$.json"
  node "$lifecycle" stop "$state_file" "$runtime_root/runtime" "$runtime"
  bin_old="$PREFIX/bin/.yeet-old-$$"
  [ ! -e "$PREFIX/bin/yeet" ] || mv "$PREFIX/bin/yeet" "$bin_old"
  [ ! -e "$runtime_root/runtime" ] || mv "$runtime_root/runtime" "$runtime_old"
  if mv "$runtime_new" "$runtime_root/runtime" && mv "$bin_tmp" "$PREFIX/bin/yeet"; then
    rm -rf "$runtime_old"
    rm -f "$bin_old"
  else
    rm -rf "$runtime_root/runtime"
    [ ! -e "$runtime_old" ] || mv "$runtime_old" "$runtime_root/runtime"
    [ ! -e "$bin_old" ] || mv "$bin_old" "$PREFIX/bin/yeet"
    die "replacement failed; previous installation restored (restart settings: $state_file)"
  fi
  node "$lifecycle" replace-copies "$state_file" "$PREFIX/bin/yeet"
  node "$lifecycle" restart "$state_file" "$PREFIX/bin/yeet"
  rm -f "$state_file"

  for file in README.md CHANGELOG.md LICENSE.txt; do
    [ ! -f "$docs/$file" ] || cp "$docs/$file" "$PREFIX/share/doc/yeet/$file"
  done

  [ -f "$PREFIX/share/yeet/runtime/dist/bridge.js" ] || die "installed Yeet runtime is incomplete"
  configure_linux_path

  version=$("$PREFIX/bin/yeet" --version 2>/dev/null || printf unknown)
  printf 'Installed Yeet %s to %s/bin/yeet\n' "$version" "$PREFIX"
  printf 'Installed Yeet runtime to %s/share/yeet/runtime\n' "$PREFIX"

  if [ "$(uname -s)" != Linux ]; then
    case ":${PATH:-}:" in
      *":$PREFIX/bin:"*) ;;
      *) printf 'Add %s/bin to PATH.\n' "$PREFIX" ;;
    esac
  fi
}

install_bundle() {
  bundle=$1
  install_artifacts "$bundle/bin/yeet" "$bundle/share/yeet/runtime" "$bundle"
}

script_directory() {
  script=$0
  case "$script" in
    /*) ;;
    */*) script="$(pwd)/$script" ;;
    *) script=$(command -v "$script" 2>/dev/null || true) ;;
  esac
  [ -n "$script" ] || return 1
  [ -f "$script" ] || return 1
  CDPATH= cd -- "$(dirname -- "$script")" 2>/dev/null && pwd
}

source_dir() {
  dir=$(script_directory) || return 1
  [ -f "$dir/Cargo.toml" ] || return 1
  [ -f "$dir/RuntimeSource/package.json" ] || return 1
  [ -f "$dir/web/package.json" ] || return 1
  printf '%s\n' "$dir"
}

install_source() {
  root=$1
  need cargo
  need npm
  check_node

  printf 'Building Yeet and runtime from %s...\n' "$root"
  (
    cd "$root"
    npm --prefix RuntimeSource ci
    npm --prefix web ci
    npm --prefix RuntimeSource run build
    npm --prefix web run build
    cargo build --release
  )

  install_artifacts "$root/target/release/yeet" "$root/RuntimeSource" "$root"
}

bundle_dir() {
  dir=$(script_directory) || return 1
  [ -x "$dir/bin/yeet" ] || return 1
  [ -f "$dir/share/yeet/runtime/dist/bridge.js" ] || return 1
  printf '%s\n' "$dir"
}

BUILD_FROM_SOURCE=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --build)
      BUILD_FROM_SOURCE=1
      ;;
    -h|--help)
      cat <<'EOF'
Usage: ./install.sh [--build]

Without options, download and install the latest Yeet release.

  --build    Build source, stop Yeet and its runtime/MCP children, replace all
             artifacts, and restart background, remote, and HTTP MCP services.
  -h|--help Show this help.
EOF
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
  shift
done

if [ "$BUILD_FROM_SOURCE" -eq 1 ]; then
  source=$(source_dir) || die "--build requires a Yeet source checkout"
  install_source "$source"
  exit 0
fi

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

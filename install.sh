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

kill_yeet_linux_proc() {
  signal=$1
  for proc in /proc/[0-9]*; do
    [ -r "$proc/comm" ] || continue
    IFS= read -r name < "$proc/comm" || continue
    case "$name" in
      yeet|YeetRemote|yeet-remote)
        pid=${proc##*/}
        kill "-$signal" "$pid" >/dev/null 2>&1 || true
        ;;
    esac
  done
}

kill_yeet() {
  names="yeet YeetRemote yeet-remote"
  if command -v pkill >/dev/null 2>&1; then
    for name in $names; do
      pkill -TERM -x "$name" >/dev/null 2>&1 || true
    done
    sleep 1
    for name in $names; do
      pkill -KILL -x "$name" >/dev/null 2>&1 || true
    done
    return
  fi

  # Minimal Linux images often have /proc and a shell builtin `kill`, but no
  # procps package (and therefore no pkill/pgrep). Do not leave an old Yeet
  # daemon alive merely because those optional utilities are absent.
  if [ -d /proc ]; then
    kill_yeet_linux_proc TERM
    sleep 1
    kill_yeet_linux_proc KILL
    return
  fi

  if command -v pgrep >/dev/null 2>&1; then
    for signal in TERM KILL; do
      for name in $names; do
        pgrep -x "$name" 2>/dev/null | while IFS= read -r pid; do
          [ -n "$pid" ] && kill "-$signal" "$pid" >/dev/null 2>&1 || true
        done
      done
      [ "$signal" = TERM ] && sleep 1
    done
  fi
}

kill_yeet

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

need cargo
need node
need npm

cd "$ROOT"
npm --prefix RuntimeSource ci
npm --prefix web ci
npm --prefix RuntimeSource run build
npm --prefix web run build
cargo build --release

YEET_SOURCE_ROOT="$ROOT" "$ROOT/target/release/yeet" install binary runtime

PREFIX=${YEET_PREFIX:-${PREFIX:-"$HOME/.local"}}
printf '\nYeet is installed at %s/bin/yeet\n' "$PREFIX"
case ":${PATH:-}:" in
  *":$PREFIX/bin:"*) ;;
  *) printf 'Add %s/bin to PATH.\n' "$PREFIX" ;;
esac

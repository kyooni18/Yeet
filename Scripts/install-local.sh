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
# Cargo already builds crates in parallel; make the job count explicit and
# allow callers to reduce it on memory-constrained machines.
build_jobs=${CARGO_BUILD_JOBS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.logicalcpu 2>/dev/null || printf '1')}
printf 'Building Rust release with %s parallel jobs\n' "$build_jobs"
# These builds have separate outputs; overlap them without changing the install
# layout or replacing a running installation until both have succeeded.
YEET_RUNTIME_OUT_DIR="$RUNTIME_BUILD/dist" "$ROOT/Scripts/rebuild-runtime.sh" &
runtime_pid=$!
build_status=0
cargo build --release --jobs "$build_jobs" || build_status=$?
wait "$runtime_pid" || build_status=$?
[ "$build_status" -eq 0 ] || exit "$build_status"
mkdir -p "$DEST" "$RUNTIME_DEST"

RUNNING_MCP_PORTS=""
if [ -x "$DEST/yeet" ]; then
  RUNNING_MCP_PORTS=$("$DEST/yeet" mcpserver list 2>/dev/null | awk '$2 == "running" { print $1 }' || true)
  for port in $RUNNING_MCP_PORTS; do
    echo "Stopping Yeet MCP daemon on port $port before replacing the executable"
    "$DEST/yeet" mcpserver stop --port "$port" >/dev/null
  done
fi

install -m 755 target/release/yeet "$DEST/yeet"
if ! cmp -s target/release/yeet "$DEST/yeet"; then
  echo "Installed yeet binary does not match the freshly built Rust release." >&2
  exit 1
fi
rm -rf "$RUNTIME_DEST/dist" "$RUNTIME_DEST/skills"
cp -R "$RUNTIME_BUILD/dist" "$RUNTIME_DEST/dist"
cp -R RuntimeSource/skills "$RUNTIME_DEST/skills"
cp RuntimeSource/package.json "$RUNTIME_DEST/package.json"
"$DEST/yeet" skyline setup >/dev/null

# Resume every daemon that was running before the install using the freshly
# replaced executable and runtime. Stopping before replacement also avoids a
# race with the daemon's executable-change watcher.
for port in $RUNNING_MCP_PORTS; do
  echo "Starting Yeet MCP daemon on port $port after local install"
  "$DEST/yeet" mcpserver start --port "$port" >/dev/null
done

echo "Install prefix: $PREFIX"
echo "Installed yeet to $DEST/yeet"
echo "Installed runtime to $RUNTIME_DEST"

case ":${PATH:-}:" in
  *":$DEST:"*) ;;
  *) echo "Note: add $DEST to PATH to run yeet by name." ;;
esac

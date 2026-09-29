#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
IMAGE=${YEET_DOCKER_IMAGE:-yeet:linux-container-smoke}
DOCKER=${DOCKER:-docker}

cd "$ROOT"
"$DOCKER" build --tag "$IMAGE" .


# The repository installer builds from source, installs RuntimeSource, and
# persists the user-local binary directory into PATH on Linux.
SOURCE_INSTALL_IMAGE="${IMAGE}-source-install"
cleanup_source_install_image() {
  "$DOCKER" image rm "$SOURCE_INSTALL_IMAGE" >/dev/null 2>&1 || true
}
trap cleanup_source_install_image EXIT INT TERM
"$DOCKER" build --target build --tag "$SOURCE_INSTALL_IMAGE" .
"$DOCKER" run --rm --entrypoint /bin/sh \
  -e HOME=/tmp/yeet-installer-home \
  -e SHELL=/bin/bash \
  "$SOURCE_INSTALL_IMAGE" -c '
    set -eu
    mkdir -p "$HOME"
    PREFIX=/tmp/yeet-prefix ./install.sh --build >/tmp/yeet-install.log
    test -x /tmp/yeet-prefix/bin/yeet
    test -f /tmp/yeet-prefix/share/yeet/runtime/dist/bridge.js
    test -f /tmp/yeet-prefix/share/yeet/runtime/package.json
    test -d /tmp/yeet-prefix/share/yeet/runtime/skills
    grep -F "/tmp/yeet-prefix/bin" "$HOME/.profile" >/dev/null
    grep -F "/tmp/yeet-prefix/bin" "$HOME/.bashrc" >/dev/null
    PATH="/tmp/yeet-prefix/bin:$PATH" yeet doctor >/dev/null
  '
cleanup_source_install_image
trap - EXIT INT TERM

# Host IDs can collide with pre-existing Debian users/groups. Build once with
# nobody:nogroup to ensure bind-mount ID matching remains collision-safe.
IDMAP_IMAGE="${IMAGE}-idmap"
"$DOCKER" build --tag "$IDMAP_IMAGE" \
  --build-arg YEET_UID=65534 \
  --build-arg YEET_GID=65534 .
"$DOCKER" run --rm --entrypoint /bin/sh "$IDMAP_IMAGE" -c '
  set -eu
  test "$(id -u)" = "65534"
  test "$(id -g)" = "65534"
  test -w "$HOME"
  test -w /workspace
'
"$DOCKER" image rm "$IDMAP_IMAGE" >/dev/null 2>&1 || true

# The packaged runtime must be able to start its Node bridge on a clean image.
"$DOCKER" run --rm "$IMAGE" doctor
# The runtime user and config directory must be writable without root.
"$DOCKER" run --rm --entrypoint /bin/sh "$IMAGE" -c '
  set -eu
  test "$(id -u)" -ne 0
  yeet model set openai/container-smoke >/dev/null
  test -s "$YEET_CONFIG_DIR/config.json"
'

# A fresh named config volume must remain writable and persist state across
# container recreation under the non-root runtime user.
CONFIG_VOLUME="yeet-config-smoke-$$"
cleanup_config_volume() {
  "$DOCKER" volume rm -f "$CONFIG_VOLUME" >/dev/null 2>&1 || true
}
trap cleanup_config_volume EXIT INT TERM
"$DOCKER" volume create "$CONFIG_VOLUME" >/dev/null
"$DOCKER" run --rm \
  -v "$CONFIG_VOLUME:/home/node/.config/yeet" \
  "$IMAGE" model set openai/volume-smoke >/dev/null
CONFIG_MODEL=$("$DOCKER" run --rm \
  -v "$CONFIG_VOLUME:/home/node/.config/yeet" \
  "$IMAGE" model get)
test "$CONFIG_MODEL" = "openai/volume-smoke"
# The packaged runtime should not require writes to the image filesystem.
"$DOCKER" run --rm --read-only \
  --tmpfs /tmp \
  --tmpfs /workspace \
  -v "$CONFIG_VOLUME:/home/node/.config/yeet" \
  "$IMAGE" doctor >/dev/null
cleanup_config_volume
trap - EXIT INT TERM

# Exercise the packaged Linux Landlock helper through the real Yeet binary. Some
# container kernels intentionally omit Landlock; that case must fail closed with
# an actionable diagnostic rather than silently losing isolation.
"$DOCKER" run --rm --entrypoint /bin/sh "$IMAGE" -c '
  set -eu
  cat > /tmp/yeet-sandbox-plan.json <<'\''JSON'\''
{"grants":[{"path":"/","writable":false}],"block_network":true}
JSON
  set +e
  output=$(yeet __linux-sandbox-shell /tmp/yeet-sandbox-plan.json '\''printf yeet-linux-sandbox'\'' 2>&1)
  sandbox_status=$?
  set -e
  if [ "$sandbox_status" -eq 0 ]; then
    test "$output" = "yeet-linux-sandbox"
    if yeet __linux-sandbox-shell /tmp/yeet-sandbox-plan.json '\''touch /workspace/yeet-sandbox-write-probe'\'' >/dev/null 2>&1; then
      echo "Linux sandbox unexpectedly allowed a write through a read-only grant." >&2
      exit 1
    fi
    test ! -e /workspace/yeet-sandbox-write-probe
  else
    printf '\''%s\n'\'' "$output" | grep -F "Linux sandbox unavailable:" >/dev/null
    printf '\''%s\n'\'' "$output" | grep -F "fails closed" >/dev/null
  fi
'


# A non-interactive container must fail immediately with an actionable error,
# rather than attempting raw-mode terminal setup and leaving background state.
set +e
NO_TTY_OUTPUT=$("$DOCKER" run --rm "$IMAGE" 2>&1)
NO_TTY_STATUS=$?
set -e
if [ "$NO_TTY_STATUS" -eq 0 ]; then
  echo "Expected a no-TTY Yeet launch to fail." >&2
  exit 1
fi
printf '%s\n' "$NO_TTY_OUTPUT" | grep -F "Yeet TUI requires an interactive terminal" >/dev/null

# Long-lived services must stay in the container foreground through the image's
# actual tini -> yeet entrypoint. Wildcard binds require an explicit public URL.
MCP_CONTAINER="yeet-mcp-smoke-$$"
cleanup_mcp() {
  "$DOCKER" rm -f "$MCP_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup_mcp EXIT INT TERM
"$DOCKER" run -d --name "$MCP_CONTAINER" "$IMAGE" \
  mcpserver run \
  --bind 0.0.0.0 \
  --port 7332 \
  --public-url http://localhost:7332/mcp >/dev/null
attempt=0
while [ "$attempt" -lt 80 ]; do
  if "$DOCKER" exec "$MCP_CONTAINER" node -e 'fetch("http://127.0.0.1:7332/health").then(r => process.exit(r.ok ? 0 : 1)).catch(() => process.exit(1))' >/dev/null 2>&1; then
    break
  fi
  running=$("$DOCKER" inspect -f '{{.State.Running}}' "$MCP_CONTAINER" 2>/dev/null || printf 'false')
  if [ "$running" != "true" ]; then
    "$DOCKER" logs "$MCP_CONTAINER" >&2 || true
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
if [ "$attempt" -ge 80 ]; then
  "$DOCKER" logs "$MCP_CONTAINER" >&2 || true
  exit 1
fi
"$DOCKER" stop -t 3 "$MCP_CONTAINER" >/dev/null
MCP_STATE=$("$DOCKER" inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$MCP_CONTAINER")
case "$MCP_STATE" in
  "exited 0"|"exited 143") ;;
  *)
    "$DOCKER" logs "$MCP_CONTAINER" >&2 || true
    echo "Unexpected MCP container stop state: $MCP_STATE" >&2
    exit 1
    ;;
esac
cleanup_mcp
trap - EXIT INT TERM

printf 'Yeet Linux container smoke passed: %s\n' "$IMAGE"

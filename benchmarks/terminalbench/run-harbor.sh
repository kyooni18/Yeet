#!/usr/bin/env bash
set -euo pipefail

ROOT="/opt/yeet-benchmark"
export PYTHONPATH="$ROOT${PYTHONPATH:+:$PYTHONPATH}"
export PATH="/root/.local/bin:/usr/local/bin:$PATH"
export HOME=/root
unset DOCKER_CONTEXT
export DOCKER_HOST=unix:///var/run/docker.sock

if ! docker info >/dev/null 2>&1; then
  echo "Dedicated Terminal-Bench Docker daemon is unavailable." >&2
  exit 2
fi

PREFLIGHT_VOLUME="yeet-terminalbench-preflight-$$"
docker volume create "$PREFLIGHT_VOLUME" >/dev/null
docker volume rm "$PREFLIGHT_VOLUME" >/dev/null

# Keep the dedicated sparse disk bounded during long TB2 runs. This daemon has
# no non-benchmark workloads, so unused task images can be pruned aggressively.
janitor() {
  while sleep 120; do
    docker image prune -af >/dev/null 2>&1 || true
    docker builder prune -af >/dev/null 2>&1 || true
    fstrim -av >/dev/null 2>&1 || true
  done
}
janitor &
JANITOR_PID=$!
cleanup() {
  kill "$JANITOR_PID" >/dev/null 2>&1 || true
  wait "$JANITOR_PID" >/dev/null 2>&1 || true
  docker image prune -af >/dev/null 2>&1 || true
  docker builder prune -af >/dev/null 2>&1 || true
  fstrim -av >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

set +e
/root/.local/bin/harbor run \
  --dataset terminal-bench@2.0 \
  --agent benchmarks.terminalbench.yeet_agent:YeetAgent \
  "$@"
rc=$?
set -e
exit "$rc"

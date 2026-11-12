#!/usr/bin/env bash
# Run Terminal-Bench from macOS using host Harbor against a dedicated Rosetta-enabled dind daemon
# (container `tb-dockerd`, socket ~/.docker/tb-dockerd.sock). Never touches the normal Docker daemon.
#
# One-time daemon setup:
#   container volume create tb-docker-data
#   container run -d --name tb-dockerd --rosetta --cap-add ALL --masked-path NONE --read-only-path NONE \
#     -c 4 -m 8G -e DOCKER_TLS_CERTDIR= -v tb-docker-data:/var/lib/docker --tmpfs /run/docker \
#     --publish-socket "$HOME/.docker/tb-dockerd.sock:/var/run/docker.sock" \
#     docker:29.7.2-dind dockerd-entrypoint.sh dockerd -H unix:///var/run/docker.sock
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export DOCKER_HOST="unix://${YEET_TB_DOCKER_SOCK:-$HOME/.docker/tb-dockerd.sock}"
export PYTHONPATH="$ROOT${PYTHONPATH:+:$PYTHONPATH}"

docker info >/dev/null 2>&1 || { echo "Benchmark Docker daemon unreachable at $DOCKER_HOST" >&2; exit 2; }

cd "$ROOT"
exec harbor run \
  --dataset "${YEET_TB_DATASET:-terminal-bench/terminal-bench-2-1}" \
  --agent benchmarks.terminalbench.yeet_agent:YeetAgent \
  "$@"

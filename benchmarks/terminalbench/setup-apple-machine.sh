#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MACHINE="${YEET_TERMINALBENCH_MACHINE:-yeet-terminalbench}"
CPUS="${YEET_TERMINALBENCH_CPUS:-4}"
MEMORY="${YEET_TERMINALBENCH_MEMORY:-8G}"
SYNC_BENCHMARKS=1
if [[ "${1:-}" == "--no-sync" ]]; then
  SYNC_BENCHMARKS=0
fi

if ! command -v container >/dev/null 2>&1; then
  echo "Apple container CLI is required." >&2
  exit 2
fi

if ! container machine inspect "$MACHINE" >/dev/null 2>&1; then
  container machine create dockerd:latest \
    -n "$MACHINE" \
    --cpus "$CPUS" \
    --memory "$MEMORY" \
    --home-mount none \
    --progress plain >/dev/null
fi

status="$(container machine inspect "$MACHINE" | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["status"])')"
if [[ "$status" != "running" ]]; then
  container machine run -n "$MACHINE" -d --root -- /usr/bin/sleep infinity >/dev/null
fi

if ! container machine run -n "$MACHINE" --root -- /usr/bin/pgrep -x sleep >/dev/null 2>&1; then
  container machine run -n "$MACHINE" -d --root -- /usr/bin/sleep infinity >/dev/null
fi

need_packages=0
for path in /usr/bin/python3 /usr/bin/curl /usr/bin/git /root/.local/bin/harbor; do
  if ! container machine run -n "$MACHINE" --root -- test -x "$path" >/dev/null 2>&1; then
    need_packages=1
    break
  fi
done
if ! container machine run -n "$MACHINE" --root -- /usr/bin/docker compose version >/dev/null 2>&1; then
  need_packages=1
fi

if (( need_packages )); then
  container machine run -n "$MACHINE" --root -- /usr/bin/apt-get update
  container machine run -n "$MACHINE" --root -- /usr/bin/env DEBIAN_FRONTEND=noninteractive \
    /usr/bin/apt-get install -y python3 python3-venv curl git ca-certificates docker-compose-plugin
  if ! container machine run -n "$MACHINE" --root -- test -x /usr/local/bin/uv >/dev/null 2>&1; then
    container machine run -n "$MACHINE" --root -- /usr/bin/curl -LsSf \
      -o /tmp/uv-install.sh https://astral.sh/uv/install.sh
    container machine run -n "$MACHINE" --root -- /usr/bin/env UV_INSTALL_DIR=/usr/local/bin \
      /bin/sh /tmp/uv-install.sh
  fi
  container machine run -n "$MACHINE" --root -- /usr/local/bin/uv tool install --force harbor
fi

# Terminal-Bench 2 contains amd64-only task images. Register emulation only in
# this dedicated machine; never in the user's normal Docker VM.
if ! container machine run -n "$MACHINE" --root -- \
  test -r /proc/sys/fs/binfmt_misc/qemu-x86_64 >/dev/null 2>&1; then
  container machine run -n "$MACHINE" --root -- \
    /usr/bin/docker run --privileged --rm tonistiigi/binfmt --install amd64 >/dev/null
fi

container machine run -n "$MACHINE" --root -- /usr/bin/docker info >/dev/null
container machine run -n "$MACHINE" --root -- /usr/bin/docker compose version >/dev/null

if (( SYNC_BENCHMARKS )); then
  container machine run -n "$MACHINE" --root -- mkdir -p /opt/yeet-benchmark
  tar --no-xattrs \
    --exclude='benchmarks/terminalbench/__pycache__' \
    -cf - benchmarks | \
    container machine run -n "$MACHINE" -i --root -- tar -xf - -C /opt/yeet-benchmark
fi

echo "$MACHINE"

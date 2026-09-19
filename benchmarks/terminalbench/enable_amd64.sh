#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MACHINE="${YEET_TERMINALBENCH_MACHINE:-yeet-terminalbench}"
"$ROOT/benchmarks/terminalbench/setup-apple-machine.sh" --no-sync >/dev/null
arch="$(container machine run -n "$MACHINE" --root -- \
  /usr/bin/docker run --rm --platform linux/amd64 alpine:3.23.4 uname -m)"
[[ "$arch" == "x86_64" ]]
echo "amd64 emulation is available inside Apple machine: $MACHINE"

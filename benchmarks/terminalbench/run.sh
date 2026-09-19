#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MACHINE="${YEET_TERMINALBENCH_MACHINE:-yeet-terminalbench}"
MIN_FREE_GB="${YEET_HARBOR_MIN_FREE_GB:-20}"

if ! [[ "$MIN_FREE_GB" =~ ^[0-9]+$ ]]; then
  echo "YEET_HARBOR_MIN_FREE_GB must be a non-negative integer (got: $MIN_FREE_GB)." >&2
  exit 2
fi
FREE_KB="$(df -Pk "$ROOT" | awk 'NR == 2 {print $4}')"
MIN_FREE_KB=$((MIN_FREE_GB * 1024 * 1024))
if [[ "$FREE_KB" =~ ^[0-9]+$ ]] && (( FREE_KB < MIN_FREE_KB )); then
  FREE_GB=$((FREE_KB / 1024 / 1024))
  echo "Only about ${FREE_GB} GiB is free on the host filesystem." >&2
  echo "Refusing to grow the isolated Terminal-Bench sparse disk below the ${MIN_FREE_GB} GiB safety floor." >&2
  echo "Override deliberately with YEET_HARBOR_MIN_FREE_GB=0." >&2
  exit 2
fi

"$ROOT/benchmarks/terminalbench/setup-apple-machine.sh" >/dev/null

env_args=()
for key in \
  OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY GOOGLE_API_KEY \
  OPENROUTER_API_KEY OPENCODE_API_KEY \
  HTTP_PROXY HTTPS_PROXY NO_PROXY ALL_PROXY; do
  if [[ -n "${!key:-}" ]]; then
    env_args+=(--env "$key")
  fi
done

set +e
container machine run -n "$MACHINE" --root \
  "${env_args[@]}" \
  -w /opt/yeet-benchmark -- \
  /opt/yeet-benchmark/benchmarks/terminalbench/run-harbor.sh "$@"
rc=$?
set -e

# Bring benchmark reports back without mounting the host home into the machine.
mkdir -p "$ROOT/jobs"
if container machine run -n "$MACHINE" --root -- test -d /opt/yeet-benchmark/jobs >/dev/null 2>&1; then
  container machine run -n "$MACHINE" --root -- \
    tar -C /opt/yeet-benchmark -cf - jobs | tar -C "$ROOT" -xf -
fi

exit "$rc"

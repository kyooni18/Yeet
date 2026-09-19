#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MACHINE="${YEET_TERMINALBENCH_MACHINE:-yeet-terminalbench}"
ARCH="${1:-}"
if [[ -z "$ARCH" ]]; then
  case "$(uname -m)" in
    arm64|aarch64) ARCH=arm64 ;;
    x86_64|amd64) ARCH=amd64 ;;
    *) echo "Unsupported host architecture; specify arm64 or amd64." >&2; exit 2 ;;
  esac
fi
case "$ARCH" in
  arm64|amd64) ;;
  *) echo "Usage: $0 [arm64|amd64]" >&2; exit 2 ;;
esac

"$ROOT/benchmarks/terminalbench/setup-apple-machine.sh" --no-sync >/dev/null

container machine run -n "$MACHINE" --root -- rm -rf /opt/yeet-build-src /opt/yeet-build-out
container machine run -n "$MACHINE" --root -- mkdir -p /opt/yeet-build-src

tar --no-xattrs \
  --exclude='./.git' \
  --exclude='./target' \
  --exclude='./jobs' \
  --exclude='./benchmarks/terminalbench/dist' \
  --exclude='./.DS_Store' \
  --exclude='./.tmp*' \
  -cf - . | \
  container machine run -n "$MACHINE" -i --root -- tar -xf - -C /opt/yeet-build-src

container machine run -n "$MACHINE" --root -w /opt/yeet-build-src -- \
  /opt/yeet-build-src/benchmarks/terminalbench/build-bundle-inner.sh "$ARCH"

OUT="$ROOT/benchmarks/terminalbench/dist/linux-$ARCH"
mkdir -p "$OUT"
container machine run -n "$MACHINE" --root -- \
  cat "/opt/yeet-build-out/linux-$ARCH/yeet-bundle.tar.gz" > "$OUT/yeet-bundle.tar.gz"
container machine run -n "$MACHINE" --root -- \
  cat "/opt/yeet-build-out/linux-$ARCH/yeet-bundle.tar.gz.sha256" > "$OUT/yeet-bundle.tar.gz.sha256"

# Rewrite the checksum path to the local artifact name and verify it on macOS.
hash="$(awk '{print $1}' "$OUT/yeet-bundle.tar.gz.sha256")"
printf '%s  %s\n' "$hash" "yeet-bundle.tar.gz" > "$OUT/yeet-bundle.tar.gz.sha256"
(
  cd "$OUT"
  shasum -a 256 -c yeet-bundle.tar.gz.sha256
)

container machine run -n "$MACHINE" --root -- /usr/bin/docker image rm -f "yeet-terminalbench-bundle:$ARCH" >/dev/null 2>&1 || true
container machine run -n "$MACHINE" --root -- /usr/sbin/fstrim -av >/dev/null 2>&1 || true

echo "$OUT/yeet-bundle.tar.gz"

#!/usr/bin/env bash
set -euo pipefail

ROOT=/opt/yeet-build-src
ARCH="${1:-}"
case "$ARCH" in
  arm64)
    PLATFORM=linux/arm64
    RUST_TARGET=aarch64-unknown-linux-musl
    DOCKERFILE="$ROOT/benchmarks/terminalbench/Dockerfile.bundle"
    ;;
  amd64)
    PLATFORM=linux/amd64
    RUST_TARGET=x86_64-unknown-linux-musl
    DOCKERFILE="$ROOT/benchmarks/terminalbench/Dockerfile.bundle.amd64"
    ;;
  *) echo "Usage: $0 [arm64|amd64]" >&2; exit 2 ;;
esac

IMAGE="yeet-terminalbench-bundle:$ARCH"
OUT="/opt/yeet-build-out/linux-$ARCH"
TMP="$(mktemp -d)"
CID=""
cleanup() {
  [[ -n "$CID" ]] && docker rm -f "$CID" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

rm -rf "$OUT"
mkdir -p "$OUT"
docker build \
  --platform "$PLATFORM" \
  -f "$DOCKERFILE" \
  --build-arg "RUST_TARGET=$RUST_TARGET" \
  --target bundle \
  -t "$IMAGE" \
  "$ROOT"
CID="$(docker create --platform "$PLATFORM" "$IMAGE" /opt/yeet/yeet --version)"
docker cp "$CID:/opt/yeet/." "$TMP/"
tar -C "$TMP" -czf "$OUT/yeet-bundle.tar.gz" .
sha256sum "$OUT/yeet-bundle.tar.gz" > "$OUT/yeet-bundle.tar.gz.sha256"

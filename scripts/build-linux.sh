#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DIST_DIR="$PROJECT_DIR/dist"
IMAGE="keytao-app-builder"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found. Install Docker: https://docs.docker.com/engine/install/" >&2
  exit 1
fi

mkdir -p "$DIST_DIR"

echo "==> Building builder image..."
docker build -f "$SCRIPT_DIR/Dockerfile.linux-builder" -t "$IMAGE" "$PROJECT_DIR"

echo "==> Building deb + tar.gz inside container..."
_uid=$(id -u)
_gid=$(id -g)
docker run --rm \
  --network=host \
  -v "$PROJECT_DIR":/app \
  -v keytao-app-cargo:/root/.cargo/registry \
  -v keytao-app-cargo-git:/root/.cargo/git \
  -w /app \
  "$IMAGE" \
  sh /app/scripts/container-build.sh "$_uid" "$_gid"

echo ""
echo "==> Artifacts:"
ls -lh "$DIST_DIR"/*.deb "$DIST_DIR"/*.tar.gz 2>/dev/null \
  || ls -lh "$PROJECT_DIR"/target/release/bundle/deb/*.deb \
            "$PROJECT_DIR"/target/release/bundle/*.tar.gz 2>/dev/null \
  || echo "(check target/release/bundle/)"

exit 0

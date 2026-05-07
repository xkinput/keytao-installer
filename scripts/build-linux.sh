#!/usr/bin/env bash
set -euo pipefail

IMAGE="keytao-appimage-env"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TAURI_CACHE="$HOME/.cache/tauri"
LINUXDEPLOY_EXTRACTED="$HOME/.cache/tauri-extracted"
TARGET_VOLUME="keytao-appimage-target"

# Pre-download Tauri AppImage tools on the host (better network access than container)
mkdir -p "$TAURI_CACHE"
if [ ! -f "$TAURI_CACHE/AppRun-x86_64" ]; then
  echo "==> Downloading AppRun-x86_64..."
  curl -fSL "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64" \
    -o "$TAURI_CACHE/AppRun-x86_64"
  chmod +x "$TAURI_CACHE/AppRun-x86_64"
fi
if [ ! -f "$TAURI_CACHE/linuxdeploy-x86_64.AppImage" ]; then
  echo "==> Downloading linuxdeploy-x86_64.AppImage..."
  curl -fSL "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/linuxdeploy-x86_64.AppImage" \
    -o "$TAURI_CACHE/linuxdeploy-x86_64.AppImage"
  chmod +x "$TAURI_CACHE/linuxdeploy-x86_64.AppImage"
fi

# Pre-extract linuxdeploy so it works without FUSE inside Docker
if [ ! -f "$LINUXDEPLOY_EXTRACTED/AppRun" ]; then
  echo "==> Extracting linuxdeploy for container use..."
  mkdir -p "$LINUXDEPLOY_EXTRACTED"
  cp "$TAURI_CACHE/linuxdeploy-x86_64.AppImage" /tmp/_ld_extract.AppImage
  cd /tmp && APPIMAGE_EXTRACT_AND_RUN=1 /tmp/_ld_extract.AppImage --appimage-extract > /dev/null 2>&1
  cp -r /tmp/squashfs-root/. "$LINUXDEPLOY_EXTRACTED/"
  cd - > /dev/null
  echo "==> linuxdeploy extracted to $LINUXDEPLOY_EXTRACTED"
fi

echo "==> Building Docker build environment..."
docker build -f "$PROJECT_DIR/Dockerfile.appimage" -t "$IMAGE" "$PROJECT_DIR"

echo "==> Running AppImage build..."
docker run --rm \
  -e CI=true \
  -e APPIMAGE_EXTRACT_AND_RUN=1 \
  -v "$PROJECT_DIR:/app" \
  -v "$HOME/.cargo/registry:/root/.cargo/registry" \
  -v "$TARGET_VOLUME:/app/target" \
  -v "$TAURI_CACHE:/mnt/tauri-cache:ro" \
  -v "$LINUXDEPLOY_EXTRACTED:/opt/linuxdeploy" \
  -w /app \
  "$IMAGE" \
  bash /app/scripts/_docker-build.sh

echo ""
echo "==> Done! Copying packages to $PROJECT_DIR/dist/..."
mkdir -p "$PROJECT_DIR/dist"
docker run --rm \
  -v "$TARGET_VOLUME:/app/target" \
  -v "$PROJECT_DIR/dist:/out" \
  ubuntu:22.04 \
  bash -c "
    cp /app/target/release/bundle/appimage/*.AppImage /out/ 2>/dev/null && echo 'AppImage copied.' || echo 'No AppImage found.'
    cp /app/target/release/bundle/deb/*.deb /out/ 2>/dev/null && echo 'deb copied.' || echo 'No deb found.'
  "
ls -lh "$PROJECT_DIR"/dist/ 2>/dev/null || echo "  (check dist/)"

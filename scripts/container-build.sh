#!/bin/sh
# Runs INSIDE the Docker container. Called by build-linux.sh via docker run.
# Arguments: $1=uid $2=gid
set -eu

UID_GID="$1:$2"

echo "=== Cache contents ==="
ls -lah /root/.cache/tauri/ 2>/dev/null || echo "(empty)"

chmod -R u+w target/release/bundle/ 2>/dev/null || true
pnpm install --frozen-lockfile
cargo build -p keytao-linux-ime --release
export KEYTAO_IME_PATH=/app/target/release/keytao-ime
export TAURI_CONFIG='{"bundle":{"externalBin":["binaries/keytao-ime"]}}'
pnpm tauri build --bundles deb
VERSION=$(node -p "require('./package.json').version")
tar -czf "target/release/bundle/keytao-app-${VERSION}-linux-x86_64.tar.gz" \
  -C target/release keytao-app keytao-ime
chown -R "$UID_GID" /app/target /app/dist 2>/dev/null || true

#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
XDG_CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"
TAURI_CACHE="$XDG_CACHE_HOME/tauri"
LINUXDEPLOY_EXTRACTED="$XDG_CACHE_HOME/tauri-extracted"
BUILD_CACHE_DIR="$XDG_CACHE_HOME/keytao-linux-build"
BUILD_BIN_DIR="$BUILD_CACHE_DIR/bin"
DIST_DIR="$PROJECT_DIR/dist"
UPSTREAM_LINUXDEPLOY="$TAURI_CACHE/linuxdeploy-upstream-x86_64.AppImage"
WRAPPED_LINUXDEPLOY="$TAURI_CACHE/linuxdeploy-x86_64.AppImage"
GTK_PLUGIN="$TAURI_CACHE/linuxdeploy-plugin-gtk.sh"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

require_cmd cargo
require_cmd curl
require_cmd gcc
require_cmd pnpm
require_cmd sed

if [[ -z "${NIX_LD:-}" ]]; then
  echo "NIX_LD is not set. Load the repo via direnv first: 'direnv allow' and re-enter this directory." >&2
  exit 1
fi

mkdir -p "$TAURI_CACHE" "$LINUXDEPLOY_EXTRACTED" "$BUILD_BIN_DIR" "$DIST_DIR"

if [ ! -f "$TAURI_CACHE/AppRun-x86_64" ]; then
  echo "==> Downloading AppRun-x86_64..."
  curl -fSL "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64" \
    -o "$TAURI_CACHE/AppRun-x86_64"
  chmod +x "$TAURI_CACHE/AppRun-x86_64"
fi

if [ ! -f "$UPSTREAM_LINUXDEPLOY" ]; then
  echo "==> Downloading upstream linuxdeploy AppImage..."
  curl -fSL "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/linuxdeploy-x86_64.AppImage" \
    -o "$UPSTREAM_LINUXDEPLOY"
  chmod +x "$UPSTREAM_LINUXDEPLOY"
fi

if [ ! -f "$GTK_PLUGIN" ]; then
  echo "==> Downloading linuxdeploy gtk plugin..."
  curl -fSL "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/linuxdeploy-plugin-gtk.sh" \
    -o "$GTK_PLUGIN"
  chmod +x "$GTK_PLUGIN"
fi

if [ ! -f "$LINUXDEPLOY_EXTRACTED/AppRun" ]; then
  echo "==> Extracting linuxdeploy for host use..."
  TMP_APPIMAGE="$BUILD_CACHE_DIR/linuxdeploy-extract.AppImage"
  rm -rf "$BUILD_CACHE_DIR/squashfs-root" "$TMP_APPIMAGE"
  cp "$UPSTREAM_LINUXDEPLOY" "$TMP_APPIMAGE"
  (
    cd "$BUILD_CACHE_DIR"
    APPIMAGE_EXTRACT_AND_RUN=1 "$TMP_APPIMAGE" --appimage-extract >/dev/null 2>&1
  )
  rm -rf "$LINUXDEPLOY_EXTRACTED"
  mkdir -p "$LINUXDEPLOY_EXTRACTED"
  cp -r "$BUILD_CACHE_DIR/squashfs-root/." "$LINUXDEPLOY_EXTRACTED/"
  rm -rf "$BUILD_CACHE_DIR/squashfs-root" "$TMP_APPIMAGE"
fi

cat > "$BUILD_CACHE_DIR/ld-wrapper.c" <<EOF
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    const char *real = "$LINUXDEPLOY_EXTRACTED/AppRun";
    char *args[argc + 1];
    int out = 0;

    args[out++] = (char *)real;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--appimage-extract-and-run") != 0) {
            args[out++] = argv[i];
        }
    }
    args[out] = NULL;

    setenv("LINUXDEPLOY", real, 1);
    execv(real, args);
    return 1;
}
EOF
gcc -O2 -o "$WRAPPED_LINUXDEPLOY" "$BUILD_CACHE_DIR/ld-wrapper.c"
chmod +x "$WRAPPED_LINUXDEPLOY"

cat > "$BUILD_BIN_DIR/linuxdeploy-plugin-gtk" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

XDG_CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"
plugin="$XDG_CACHE_HOME/tauri/linuxdeploy-plugin-gtk.sh"

if [ ! -f "$plugin" ]; then
  echo "Missing linuxdeploy gtk plugin at $plugin" >&2
  exit 1
fi

chmod +x "$plugin"
sed -i 's/ln \$verbose -s /ln -f \$verbose -s /g' "$plugin" 2>/dev/null || true
exec "$plugin" "$@"
EOF
chmod +x "$BUILD_BIN_DIR/linuxdeploy-plugin-gtk"

export APPIMAGE_EXTRACT_AND_RUN=1
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL="${CARGO_REGISTRIES_CRATES_IO_PROTOCOL:-sparse}"
export PATH="$BUILD_BIN_DIR:$LINUXDEPLOY_EXTRACTED/usr/bin:$PATH"

echo "==> Installing frontend dependencies..."
pnpm install

echo "==> Pre-building keytao-linux-ime..."
cargo build -p keytao-linux-ime --release
export KEYTAO_IME_PATH="$PROJECT_DIR/target/release/keytao-ime"
echo "==> keytao-ime binary: $(ls -lh "$KEYTAO_IME_PATH")"

echo "==> Building deb + AppImage on host..."
pnpm tauri build --bundles deb,appimage

echo ""
echo "==> Copying packages to $DIST_DIR/..."
shopt -s nullglob
for file in "$PROJECT_DIR"/target/release/bundle/appimage/*.AppImage "$PROJECT_DIR"/target/release/bundle/deb/*.deb; do
  cp -f "$file" "$DIST_DIR/"
done
ls -lh "$DIST_DIR" 2>/dev/null || echo "  (check dist/)"

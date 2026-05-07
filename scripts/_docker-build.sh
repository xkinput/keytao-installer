#!/usr/bin/env bash
# This script runs INSIDE the Docker container during AppImage build.
# It is called by build-appimage.sh via: bash /app/scripts/_docker-build.sh
set -euo pipefail
trap 'echo "=== LINUXDEPLOY WRAPPER LOG ==="; cat /tmp/ld-wrapper.log 2>/dev/null || echo "(no wrapper log)"' EXIT

# ── 1. Copy tauri cache to a writable location ────────────────────────────────
cp -r /mnt/tauri-cache /tmp/tauri-tools
chmod +x /tmp/tauri-tools/*.sh /tmp/tauri-tools/AppRun-x86_64 2>/dev/null || true

# ── 2. Compile ELF wrapper ────────────────────────────────────────────────────
# Tauri's prepare_tools() runs: dd if=/dev/zero bs=1 count=3 seek=8 conv=notrunc
# on the linuxdeploy AppImage. For a bash script, bytes 8-10 are inside the
# shebang (#!/bin/bash), corrupting it to #!/bin/b\0\0\0 → ENOENT on exec.
# For an ELF binary, bytes 8-10 are EI_ABIVERSION + EI_PAD (typically zero),
# so dd is a no-op.
#
# The wrapper strips --appimage-extract-and-run (which Tauri always adds) and
# execs the pre-extracted linuxdeploy at /opt/linuxdeploy/AppRun.  It also sets
# LINUXDEPLOY (if not already set) so the gtk plugin calls AppRun directly on
# its callback — a stable path that requires no FUSE.
cat > /tmp/ld-wrapper.c << 'CSRC'
#include <unistd.h>
#include <string.h>
#include <stdlib.h>
int main(int argc, char *argv[]) {
    char *a[argc + 1];
    int j = 0;
    a[j++] = "/opt/linuxdeploy/AppRun";
    for (int i = 1; i < argc; i++)
        if (strcmp(argv[i], "--appimage-extract-and-run") != 0)
            a[j++] = argv[i];
    a[j] = 0;
    setenv("LINUXDEPLOY", "/opt/linuxdeploy/AppRun", 0);
    execv("/opt/linuxdeploy/AppRun", a);
    return 1;
}
CSRC
gcc -static -O2 -o /tmp/tauri-tools/linuxdeploy-x86_64.AppImage /tmp/ld-wrapper.c
chmod +x /tmp/tauri-tools/linuxdeploy-x86_64.AppImage

# ── 3. Install gtk plugin ─────────────────────────────────────────────────────
# Copy ONLY the gtk plugin script to /usr/local/bin so linuxdeploy can find it.
# We do NOT add all of tauri-tools to PATH: that would expose
# linuxdeploy-plugin-appimage.AppImage to the extracted linuxdeploy binary,
# which would try to run it as a nested AppImage (no FUSE → SIGABRT, exit 6).
cp /tmp/tauri-tools/linuxdeploy-plugin-gtk.sh /usr/local/bin/linuxdeploy-plugin-gtk
# Patch: use 'ln -f' so the symlink step doesn't fail when the Docker volume has
# leftover artifacts from a previous build (ln without -f exits 1 if link exists).
sed -i 's/ln \$verbose -s /ln -f \$verbose -s /g' /usr/local/bin/linuxdeploy-plugin-gtk
chmod +x /usr/local/bin/linuxdeploy-plugin-gtk

# ── 4. Point Tauri's cache dir at our writable copy ──────────────────────────
mkdir -p /root/.cache
ln -sfn /tmp/tauri-tools /root/.cache/tauri
export PATH=/opt/linuxdeploy/usr/bin:$PATH

# ── 5. Build ──────────────────────────────────────────────────────────────────
pnpm install
pnpm tauri build --bundles deb,appimage

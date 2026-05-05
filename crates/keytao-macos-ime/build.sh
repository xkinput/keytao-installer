#!/usr/bin/env bash
# build.sh — builds KeyTao.app and an optional KeyTao.pkg installer
# Usage: ./build.sh [--release | --debug]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="$SCRIPT_DIR/build"
PROFILE="${1:-release}"
CARGO_PROFILE="$( [[ "$PROFILE" == "release" ]] && echo "release" || echo "debug" )"
CARGO_FLAGS="$( [[ "$PROFILE" == "release" ]] && echo "--release" || echo "" )"
APP="$BUILD_DIR/KeyTao.app"

echo "==> Building keytao-core-ffi ($CARGO_PROFILE)..."
cargo build $CARGO_FLAGS \
    --manifest-path "$WORKSPACE_DIR/Cargo.toml" \
    -p keytao-core-ffi \
    --target-dir "$WORKSPACE_DIR/target"

DYLIB_SRC="$WORKSPACE_DIR/target/$CARGO_PROFILE/libkeytao_core_ffi.dylib"
if [ ! -f "$DYLIB_SRC" ]; then
    echo "ERROR: dylib not found at $DYLIB_SRC" >&2
    exit 1
fi

echo "==> Creating app bundle skeleton..."
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Frameworks"
mkdir -p "$APP/Contents/Resources"

cp "$SCRIPT_DIR/Resources/Info.plist" "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

mkdir -p "$APP/Contents/Resources/en.lproj"
cat > "$APP/Contents/Resources/en.lproj/InfoPlist.strings" << 'LSTREOF'
CFBundleName = "KeyTao";
CFBundleDisplayName = "KeyTao";
LSTREOF
mkdir -p "$APP/Contents/Resources/zh-Hans.lproj"
cat > "$APP/Contents/Resources/zh-Hans.lproj/InfoPlist.strings" << 'LSTREOF'
CFBundleName = "键道";
CFBundleDisplayName = "键道输入法";
LSTREOF

cp "$DYLIB_SRC" "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
install_name_tool \
    -id "@rpath/libkeytao_core_ffi.dylib" \
    "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"

echo "==> Copying C header for Swift build..."
HEADER_DIR="$SCRIPT_DIR/Sources/CKeytaoCore"
mkdir -p "$HEADER_DIR"
cp "$WORKSPACE_DIR/crates/keytao-core-ffi/include/keytao_core.h" "$HEADER_DIR/"

echo "==> Building Swift IME executable..."
swiftc \
    "$SCRIPT_DIR/Sources/KeyTaoIME/"*.swift \
    -module-name KeyTaoIME \
    -disable-bridging-pch \
    -framework Cocoa \
    -framework InputMethodKit \
    -framework Carbon \
    -I "$HEADER_DIR" \
    -L "$APP/Contents/Frameworks" -lkeytao_core_ffi \
    -Xlinker -rpath -Xlinker @executable_path/../Frameworks \
    $( [[ "$PROFILE" == "release" ]] && echo "-O" || echo "-g" ) \
    -o "$APP/Contents/MacOS/KeyTaoIME"

echo "==> Signing (Apple Development cert)..."
ENTITLEMENTS="$SCRIPT_DIR/dev.entitlements.plist"
cat > "$ENTITLEMENTS" << 'ENTEOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.app-sandbox</key><false/>
<key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>
ENTEOF

APPLE_DEV_CERT=$(security find-identity -v -p codesigning 2>/dev/null \
    | grep "Apple Development" | head -1 | sed 's/.*"\(.*\)"/\1/')
if [ -n "$APPLE_DEV_CERT" ]; then
    SIGN_ID="$APPLE_DEV_CERT"
    echo "    Using cert: $SIGN_ID"
else
    SIGN_ID="-"
    echo "    WARNING: No Apple Development cert found, falling back to ad-hoc"
fi

codesign --force --sign "$SIGN_ID" --options runtime \
    --entitlements "$ENTITLEMENTS" \
    "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
codesign --force --sign "$SIGN_ID" --options runtime \
    --entitlements "$ENTITLEMENTS" \
    "$APP"

echo ""
echo "==> Build complete: $APP"

echo ""
echo "==> Building .pkg installer (installs via system_installd, no provenance xattr)..."
PKG_PAYLOAD="$BUILD_DIR/pkg_payload"
PKG_SCRIPTS="$BUILD_DIR/pkg_scripts"
rm -rf "$PKG_PAYLOAD" "$PKG_SCRIPTS"
mkdir -p "$PKG_PAYLOAD/Library/Input Methods"
mkdir -p "$PKG_SCRIPTS"

ditto "$APP" "$PKG_PAYLOAD/Library/Input Methods/KeyTao.app"

# Post-install script: register with TIS
cat > "$PKG_SCRIPTS/postinstall" << 'SCRIPTEOF'
#!/bin/bash
# Give Launch Services time to index the new bundle
sleep 2
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "/Library/Input Methods/KeyTao.app"
# Register with TIS — open the app briefly so it can call TISRegisterInputSource
open "/Library/Input Methods/KeyTao.app"
exit 0
SCRIPTEOF
chmod +x "$PKG_SCRIPTS/postinstall"

pkgbuild \
    --root "$PKG_PAYLOAD" \
    --scripts "$PKG_SCRIPTS" \
    --identifier "ink.rea.keytao-ime-installer" \
    --version "1.0.0" \
    --install-location "/" \
    "$BUILD_DIR/KeyTao.pkg" 2>&1

echo ""
echo "==> pkg complete: $BUILD_DIR/KeyTao.pkg"
echo ""
echo "To install via pkg (recommended — avoids provenance xattr):"
echo "  sudo installer -pkg \"$BUILD_DIR/KeyTao.pkg\" -target /"
echo ""
echo "Or direct install (files may have provenance xattr, TIS may not register):"
echo "  sudo rm -rf \"/Library/Input Methods/KeyTao.app\""
echo "  sudo ditto \"$APP\" \"/Library/Input Methods/KeyTao.app\""

#!/bin/sh
# Install the Rust viewer system-wide, beside the legacy PDF.js launcher.
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo 'The native app installer requires macOS' >&2
    exit 1
fi

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_DIR=/Applications
APP="$APP_DIR/ggpdf Native.app"
OLD_APP="$HOME/Applications/ggpdf Native.app"
PLISTBUDDY=/usr/libexec/PlistBuddy
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

if [ ! -f "$ROOT/pdfium/libpdfium.dylib" ]; then
    echo "Missing $ROOT/pdfium/libpdfium.dylib" >&2
    exit 1
fi

check_existing() {
    if [ -e "$1" ] || [ -L "$1" ]; then
        if [ ! -d "$1" ] || [ -L "$1" ] || \
            [ "$("$PLISTBUDDY" -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist" 2>/dev/null || true)" != 'local.jens.ggpdf.native' ]; then
            echo "Refusing to replace an unrelated application: $1" >&2
            exit 1
        fi
    fi
}
check_existing "$APP"
check_existing "$OLD_APP"

if [ ! -w "$APP_DIR" ]; then
    echo "No write access to $APP_DIR. Run the installer as a macOS admin user." >&2
    echo 'Do not run cargo as root; build first, then grant /Applications write access or install manually.' >&2
    exit 1
fi

cargo build --release --manifest-path "$ROOT/Cargo.toml"
mkdir -p "$APP_DIR"
STAGE=$(mktemp -d "$APP_DIR/.ggpdf-native-install.XXXXXX")
committed=0
placed=0
cleanup() {
    status=$?
    trap - EXIT
    if [ "$committed" -eq 0 ]; then
        if [ "$placed" -eq 1 ]; then rm -rf "$APP"; fi
        if [ -d "$STAGE/previous.app" ]; then mv "$STAGE/previous.app" "$APP"; fi
        if [ -d "$STAGE/previous-user.app" ]; then
            mv "$STAGE/previous-user.app" "$OLD_APP"
            "$LSREGISTER" -f "$OLD_APP" || true
        fi
    fi
    rm -rf "$STAGE"
    exit "$status"
}
trap cleanup EXIT

BUNDLE="$STAGE/ggpdf Native.app"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources/pdfium"
cp "$ROOT/target/release/ggpdf" "$BUNDLE/Contents/MacOS/ggpdf"
cp "$ROOT/pdfium/libpdfium.dylib" "$BUNDLE/Contents/Resources/pdfium/libpdfium.dylib"
cp -R "$ROOT/assets" "$BUNDLE/Contents/Resources/assets"
cp "$ROOT/../icon.icns" "$BUNDLE/Contents/Resources/ggpdf.icns"

cat > "$BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleIdentifier</key><string>local.jens.ggpdf.native</string>
    <key>CFBundleName</key><string>ggpdf Native</string>
    <key>CFBundleDisplayName</key><string>ggpdf Native</string>
    <key>CFBundleExecutable</key><string>ggpdf</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleIconFile</key><string>ggpdf.icns</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key><array><dict>
        <key>CFBundleTypeName</key><string>PDF document</string>
        <key>CFBundleTypeRole</key><string>Viewer</string>
        <key>LSHandlerRank</key><string>Alternate</string>
        <key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
    </dict></array>
</dict></plist>
PLIST

# Ad-hoc sign the embedded library first, then the complete app bundle.
/usr/bin/codesign --force --sign - "$BUNDLE/Contents/Resources/pdfium/libpdfium.dylib"
/usr/bin/codesign --force --sign - "$BUNDLE"
if [ -d "$APP" ]; then mv "$APP" "$STAGE/previous.app"; fi
if [ -d "$OLD_APP" ]; then
    "$LSREGISTER" -u "$OLD_APP" || true
    mv "$OLD_APP" "$STAGE/previous-user.app"
fi
mv "$BUNDLE" "$APP"
placed=1
"$LSREGISTER" -f "$APP"
committed=1
echo "Installed $APP (legacy ~/Applications/ggpdf.app unchanged)"

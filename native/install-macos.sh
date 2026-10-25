#!/bin/sh
# Install the Rust viewer beside the legacy PDF.js launcher, not over it.
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo 'The native app installer requires macOS' >&2
    exit 1
fi

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_DIR="$HOME/Applications"
APP="$APP_DIR/ggpdf Native.app"
PLISTBUDDY=/usr/libexec/PlistBuddy
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

if [ ! -f "$ROOT/pdfium/libpdfium.dylib" ]; then
    echo "Missing $ROOT/pdfium/libpdfium.dylib" >&2
    exit 1
fi

if [ -e "$APP" ] || [ -L "$APP" ]; then
    if [ ! -d "$APP" ] || [ -L "$APP" ] || \
        [ "$("$PLISTBUDDY" -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist" 2>/dev/null || true)" != 'local.jens.ggpdf.native' ]; then
        echo "Refusing to replace an unrelated application: $APP" >&2
        exit 1
    fi
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
mv "$BUNDLE" "$APP"
placed=1
"$LSREGISTER" -f "$APP"
committed=1
echo "Installed $APP (legacy ~/Applications/ggpdf.app unchanged)"

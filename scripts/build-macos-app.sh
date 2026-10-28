#!/bin/sh
# Builds a ready-to-run ggPdf.app bundle at OUT_APP from an already-built
# `target/release/ggpdf` binary. Pure bundling — no cargo build, no install
# destination, no legacy-app handling: install-macos.sh wraps this for a
# system install, and the release workflow wraps it to zip for distribution.
# Overwrites OUT_APP if it already exists.
set -eu

if [ $# -lt 2 ] || [ $# -gt 3 ]; then
    echo "usage: $0 ROOT OUT_APP [VERSION]" >&2
    exit 1
fi
ROOT=$1
BUNDLE=$2
VERSION=${3:-0.1.0}
BUNDLE_ID=local.jens.ggpdf.native

if [ ! -f "$ROOT/target/release/ggpdf" ]; then
    echo "Missing $ROOT/target/release/ggpdf (run: cargo build --release)" >&2
    exit 1
fi
if [ ! -f "$ROOT/pdfium/libpdfium.dylib" ]; then
    echo "Missing $ROOT/pdfium/libpdfium.dylib" >&2
    exit 1
fi
if [ ! -f "$ROOT/assets/icons/icon.png" ] || [ ! -f "$ROOT/assets/icons/icon.icns" ]; then
    echo "Missing $ROOT/assets/icons/icon.png or $ROOT/assets/icons/icon.icns" >&2
    exit 1
fi

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources/pdfium"
cp "$ROOT/target/release/ggpdf" "$BUNDLE/Contents/MacOS/ggpdf"
cp "$ROOT/pdfium/libpdfium.dylib" "$BUNDLE/Contents/Resources/pdfium/libpdfium.dylib"
cp -R "$ROOT/assets" "$BUNDLE/Contents/Resources/assets"
cp "$ROOT/assets/icons/icon.icns" "$BUNDLE/Contents/Resources/ggPdf.icns"
# Decoded at runtime for the Dock/app-switcher icon (see main.rs's
# load_app_icon doc comment — without this, eframe falls back to its own
# generic icon and overwrites CFBundleIconFile's Dock icon with it).
cp "$ROOT/assets/icons/icon.png" "$BUNDLE/Contents/Resources/icon.png"

cat > "$BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>ggPdf</string>
    <key>CFBundleDisplayName</key><string>ggPdf</string>
    <key>CFBundleExecutable</key><string>ggpdf</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleIconFile</key><string>ggPdf.icns</string>
    <key>NSHighResolutionCapable</key><true/>
    <!-- Without this, AppKit keeps native Open/Save panels in English
         regardless of the system language, because the bundle only
         declares (or defaults to) CFBundleDevelopmentRegion = en with no
         other supported localizations. No .lproj folders are needed:
         declaring the languages is what tells NSOpenPanel/NSSavePanel to
         follow the system language for their own built-in chrome. -->
    <key>CFBundleLocalizations</key><array><string>en</string><string>de</string></array>
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

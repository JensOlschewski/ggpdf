#!/bin/sh
# Install the Rust viewer system-wide, beside the legacy PDF.js launcher.
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo 'The native app installer requires macOS' >&2
    exit 1
fi

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_DIR=/Applications
APP="$APP_DIR/ggPdf.app"
BUNDLE_ID=local.jens.ggpdf.native
PLISTBUDDY=/usr/libexec/PlistBuddy
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

# Earlier installer runs used these names/locations for the same app
# (same bundle ID). Renaming to ggPdf.app replaces them instead of leaving
# duplicates around. Does NOT include "~/Applications/ggPdf.app": on this
# case-insensitive filesystem that path is indistinguishable from
# "~/Applications/ggpdf.app" — the unrelated, legacy PDF.js launcher this
# installer must never touch.
LEGACY_1="$APP_DIR/ggpdf Native.app"
LEGACY_2="$HOME/Applications/ggpdf Native.app"

if [ ! -f "$ROOT/pdfium/libpdfium.dylib" ]; then
    echo "Missing $ROOT/pdfium/libpdfium.dylib" >&2
    exit 1
fi
if [ ! -f "$ROOT/../icon.png" ]; then
    echo "Missing $ROOT/../icon.png (run: sips -s format png -z 256 256 icon.icns --out icon.png)" >&2
    exit 1
fi

bundle_id_of() {
    "$PLISTBUDDY" -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist" 2>/dev/null || true
}

# Refuses to touch a same-named/same-location app that isn't ours, rather
# than guessing it's safe to overwrite or move.
check_existing() {
    if [ -e "$1" ] || [ -L "$1" ]; then
        if [ ! -d "$1" ] || [ -L "$1" ] || [ "$(bundle_id_of "$1")" != "$BUNDLE_ID" ]; then
            echo "Refusing to touch an unrelated application: $1" >&2
            exit 1
        fi
    fi
}
check_existing "$APP"
check_existing "$LEGACY_1"
check_existing "$LEGACY_2"

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
LEGACY_MAP="$STAGE/legacy.map"
: > "$LEGACY_MAP"
cleanup() {
    status=$?
    trap - EXIT
    if [ "$committed" -eq 0 ]; then
        if [ "$placed" -eq 1 ]; then rm -rf "$APP"; fi
        if [ -d "$STAGE/previous.app" ]; then mv "$STAGE/previous.app" "$APP"; fi
        # Restore any legacy-named bundle moved aside before the failure.
        if [ -s "$LEGACY_MAP" ]; then
            while IFS='|' read -r backup original; do
                if [ -d "$backup" ]; then
                    mv "$backup" "$original"
                    "$LSREGISTER" -f "$original" || true
                fi
            done < "$LEGACY_MAP"
        fi
    fi
    rm -rf "$STAGE"
    exit "$status"
}
trap cleanup EXIT

BUNDLE="$STAGE/ggPdf.app"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources/pdfium"
cp "$ROOT/target/release/ggpdf" "$BUNDLE/Contents/MacOS/ggpdf"
cp "$ROOT/pdfium/libpdfium.dylib" "$BUNDLE/Contents/Resources/pdfium/libpdfium.dylib"
cp -R "$ROOT/assets" "$BUNDLE/Contents/Resources/assets"
cp "$ROOT/../icon.icns" "$BUNDLE/Contents/Resources/ggPdf.icns"
# Decoded at runtime for the Dock/app-switcher icon (see main.rs's
# load_app_icon doc comment — without this, eframe falls back to its own
# generic icon and overwrites CFBundleIconFile's Dock icon with it).
cp "$ROOT/../icon.png" "$BUNDLE/Contents/Resources/icon.png"

cat > "$BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>ggPdf</string>
    <key>CFBundleDisplayName</key><string>ggPdf</string>
    <key>CFBundleExecutable</key><string>ggpdf</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
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

if [ -d "$APP" ]; then mv "$APP" "$STAGE/previous.app"; fi

i=0
for legacy in "$LEGACY_1" "$LEGACY_2"; do
    if [ -d "$legacy" ]; then
        "$LSREGISTER" -u "$legacy" || true
        backup="$STAGE/legacy-$i.app"
        mv "$legacy" "$backup"
        printf '%s|%s\n' "$backup" "$legacy" >> "$LEGACY_MAP"
        i=$((i + 1))
    fi
done

mv "$BUNDLE" "$APP"
placed=1
"$LSREGISTER" -f "$APP"
committed=1
echo "Installed $APP (legacy ~/Applications/ggpdf.app unchanged; renamed bundles removed)"

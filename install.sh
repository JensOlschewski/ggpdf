#!/bin/sh
# Build and register the macOS adapter. Does not change the default handler.
set -eu

ROOT="$(cd "$(dirname "$0")" && pwd)"
APP_DIR="$HOME/Applications"
APP="$APP_DIR/ggpdf.app"
LEGACY_APP="$APP_DIR/PDF Preview.app"
PLISTBUDDY=/usr/libexec/PlistBuddy
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

for EXISTING in "$APP" "$LEGACY_APP"; do
    if [ -e "$EXISTING" ] || [ -L "$EXISTING" ]; then
        if [ ! -d "$EXISTING" ] || [ -L "$EXISTING" ] || \
            [ "$("$PLISTBUDDY" -c 'Print :CFBundleIdentifier' "$EXISTING/Contents/Info.plist" 2>/dev/null || true)" != 'local.jens.pdf-preview' ]; then
            echo "Refusing to replace an unrelated application: $EXISTING" >&2
            exit 1
        fi
    fi
done

SCRIPT_PATH=$(printf '%s' "$ROOT/preview.py" | sed 's/\\/\\\\/g; s/"/\\"/g')

mkdir -p "$APP_DIR"
TMP=$(mktemp -d "$APP_DIR/.ggpdf-install.XXXXXX")
committed=0
placed=0
cleanup() {
    status=$?
    trap - EXIT
    if [ "$committed" -eq 0 ]; then
        if [ "$placed" -eq 1 ]; then rm -rf "$APP"; fi
        if [ -d "$TMP/previous.app" ]; then
            mv "$TMP/previous.app" "$APP"
            "$LSREGISTER" -f "$APP" || true
        fi
    fi
    rm -rf "$TMP"
    exit "$status"
}
trap cleanup EXIT
sed "s|__GGPDF_PREVIEW_SCRIPT__|$SCRIPT_PATH|" "$ROOT/launcher.applescript" > "$TMP/launcher.applescript"

/usr/bin/osacompile -o "$TMP/ggpdf.app" "$TMP/launcher.applescript"

PLIST="$TMP/ggpdf.app/Contents/Info.plist"

plist_set() {
    "$PLISTBUDDY" -c "Set $1 $3" "$PLIST" 2>/dev/null || \
    "$PLISTBUDDY" -c "Add $1 $2 $3" "$PLIST"
}

plist_set :CFBundleIdentifier string local.jens.pdf-preview
plist_set :CFBundleName string ggpdf
plist_set :CFBundleDisplayName string ggpdf
plist_set :CFBundleIconFile string ggpdf.icns
plist_set :CFBundleShortVersionString string 1.0.0
plist_set :LSUIElement bool true

"$PLISTBUDDY" -c "Delete :CFBundleDocumentTypes" "$PLIST" 2>/dev/null || true
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes array" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0 dict" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:CFBundleTypeName string 'PDF document'" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:CFBundleTypeRole string Viewer" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:LSHandlerRank string Owner" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes array" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes:0 string com.adobe.pdf" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:CFBundleTypeExtensions array" "$PLIST"
"$PLISTBUDDY" -c "Add :CFBundleDocumentTypes:0:CFBundleTypeExtensions:0 string pdf" "$PLIST"

cp "$ROOT/icon.icns" "$TMP/ggpdf.app/Contents/Resources/ggpdf.icns"
/usr/bin/codesign --force --sign - "$TMP/ggpdf.app"
if [ -d "$APP" ]; then mv "$APP" "$TMP/previous.app"; fi
mv "$TMP/ggpdf.app" "$APP"
placed=1
"$LSREGISTER" -f "$APP"
committed=1

if [ -d "$LEGACY_APP" ]; then
    "$LSREGISTER" -u "$LEGACY_APP" || true
    rm -rf "$LEGACY_APP"
fi

echo "Installed $APP"

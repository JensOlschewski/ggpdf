#!/bin/sh
# Build and register the macOS adapter. Does not change the default handler.
set -eu

ROOT="$(cd "$(dirname "$0")" && pwd)"
APP="$HOME/Applications/PDF Preview.app"
PLISTBUDDY=/usr/libexec/PlistBuddy

if [ -e "$APP" ]; then
    echo "Application already exists: $APP" >&2
    exit 1
fi

SCRIPT_PATH=$(printf '%s' "$ROOT/preview.py" | sed 's/\\/\\\\/g; s/"/\\"/g')

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
sed "s|__GGPDF_PREVIEW_SCRIPT__|$SCRIPT_PATH|" "$ROOT/launcher.applescript" > "$TMP/launcher.applescript"

/usr/bin/osacompile -o "$APP" "$TMP/launcher.applescript"

PLIST="$APP/Contents/Info.plist"

plist_set() {
    "$PLISTBUDDY" -c "Set $1 $3" "$PLIST" 2>/dev/null || \
    "$PLISTBUDDY" -c "Add $1 $2 $3" "$PLIST"
}

plist_set :CFBundleIdentifier string local.jens.pdf-preview
plist_set :CFBundleName string "PDF Preview"
plist_set :CFBundleDisplayName string "PDF Preview"
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

/usr/bin/codesign --force --sign - "$APP"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP"

echo "Installed $APP"

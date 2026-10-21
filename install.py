#!/usr/bin/python3
"""Build and register the macOS adapter. Does not change the default handler."""
import plistlib
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
APP = Path.home() / "Applications/PDF Preview.app"
if APP.exists():
    raise SystemExit("Application already exists: " + str(APP))
script_path = str(ROOT / "preview.py").replace("\\", "\\\\").replace('"', '\\"')
with tempfile.TemporaryDirectory() as temporary:
    launcher = Path(temporary) / "launcher.applescript"
    launcher.write_text((ROOT / "launcher.applescript").read_text().replace(
        "__GGPDF_PREVIEW_SCRIPT__", script_path))
    subprocess.run(["/usr/bin/osacompile", "-o", str(APP), str(launcher)], check=True)
info_path = APP / "Contents/Info.plist"
with info_path.open("rb") as source:
    info = plistlib.load(source)
info.update({
    "CFBundleIdentifier": "local.jens.pdf-preview",
    "CFBundleName": "PDF Preview",
    "CFBundleDisplayName": "PDF Preview",
    "CFBundleShortVersionString": "1.0.0",
    "LSUIElement": True,
    "CFBundleDocumentTypes": [{
        "CFBundleTypeName": "PDF document",
        "CFBundleTypeRole": "Viewer",
        "LSHandlerRank": "Owner",
        "LSItemContentTypes": ["com.adobe.pdf"],
        "CFBundleTypeExtensions": ["pdf"],
    }],
})
with info_path.open("wb") as target:
    plistlib.dump(info, target)
subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(APP)], check=True)
subprocess.run([
    "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    "-f", str(APP),
], check=True)
print("Installed " + str(APP))

; Inno Setup script for ggPdf's Windows release installer.
;
; Built against the folder scripts/build-windows-app.ps1 already produces
; (ggpdf.exe, pdfium/pdfium.dll, assets/, icon.png) — passed in via
; /DBuildDir on the ISCC command line, not rebuilt here.
;
; AppId below is permanent from the first real release onward: changing it
; later breaks upgrade/uninstall detection for existing installs. Generated
; once with `uuidgen`. Never regenerate.
#define MyAppVersion GetEnv("GGPDF_VERSION")
#if MyAppVersion == ""
  #define MyAppVersion "0.1.0"
#endif

[Setup]
AppId={{EA636B25-3901-4AFF-9DB1-E3D79A39D747}
AppName=ggPdf
AppVersion={#MyAppVersion}
AppPublisher=Jens Olschewski
DefaultDirName={localappdata}\Programs\ggPdf
DefaultGroupName=ggPdf
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; Relative to this .iss file's own directory (scripts/), not the caller's
; working directory — ".." lands the installer at the repo root, where
; release.yml's upload-artifact step expects to find it.
OutputDir=..
OutputBaseFilename=ggPdf-windows-{#MyAppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\ggpdf.exe
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "{#BuildDir}\ggpdf.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\pdfium\pdfium.dll"; DestDir: "{app}\pdfium"; Flags: ignoreversion
Source: "{#BuildDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#BuildDir}\icon.png"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\ggPdf"; Filename: "{app}\ggpdf.exe"; IconFilename: "{app}\ggpdf.exe"

[Registry]
; Mirrors install-windows.ps1's $progId/$progKey/$command exactly.
Root: HKCU; Subkey: "Software\Classes\ggPdf.Native.PDF"; ValueType: string; ValueName: ""; ValueData: "ggPdf PDF document"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\ggPdf.Native.PDF\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ggpdf.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\ggPdf.Native.PDF\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\ggpdf.exe,0"
; "Open with", NOT the default handler — matches install-windows.ps1's own comment.
Root: HKCU; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "ggPdf.Native.PDF"; ValueData: ""; Flags: uninsdeletevalue

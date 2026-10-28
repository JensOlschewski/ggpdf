# Builds a ready-to-run ggPdf app directory at -OutDir from an already-built
# target/release/ggpdf.exe. Pure bundling - no cargo build, no install
# destination, no shortcut/registry wiring: install-windows.ps1 wraps this
# for a per-user install, and the release workflow wraps it to zip for
# distribution. Overwrites -OutDir if it already exists.
param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$OutDir
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$exe = Join-Path $Root 'target/release/ggpdf.exe'
$dll = Join-Path $Root 'pdfium/pdfium.dll'
$png = Join-Path $Root 'assets/icons/icon.png'
foreach ($file in @($exe, $dll, $png)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Missing $file"
    }
}

if (Test-Path -LiteralPath $OutDir) { Remove-Item -LiteralPath $OutDir -Recurse -Force }
New-Item -ItemType Directory -Path $OutDir | Out-Null
New-Item -ItemType Directory -Path (Join-Path $OutDir 'pdfium') | Out-Null
Copy-Item -LiteralPath $exe -Destination $OutDir
Copy-Item -LiteralPath $dll -Destination (Join-Path $OutDir 'pdfium')
Copy-Item -LiteralPath (Join-Path $Root 'assets') -Destination $OutDir -Recurse
Copy-Item -LiteralPath $png -Destination $OutDir

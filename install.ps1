# Build and install ggPdf for the current Windows user (no elevation required).
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows) {
    throw 'This installer requires Windows.'
}

$root = $PSScriptRoot
$dll = Join-Path $root 'pdfium/pdfium.dll'
$png = Join-Path $root 'assets/icons/icon.png'
foreach ($file in @($dll, $png)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Missing $file"
    }
}
if (-not $env:LOCALAPPDATA -or -not $env:APPDATA) {
    throw 'LOCALAPPDATA and APPDATA must be set for a per-user install.'
}

$parent = Join-Path $env:LOCALAPPDATA 'Programs'
$app = Join-Path $parent 'ggPdf'
$marker = 'local.jens.ggpdf.native'
$shortcut = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs/ggPdf.lnk'
$exe = Join-Path $app 'ggpdf.exe'
$progId = 'ggPdf.Native.PDF'
$progKey = "HKCU:\Software\Classes\$progId"
$openWith = 'HKCU:\Software\Classes\.pdf\OpenWithProgids'
$command = '"' + $exe + '" "%1"'

# Never overwrite an unrelated directory, shortcut or file association.
if (Test-Path -LiteralPath $app) {
    $item = Get-Item -LiteralPath $app -Force
    if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        -not (Test-Path -LiteralPath (Join-Path $app $marker) -PathType Leaf) -or
        (Get-Content -LiteralPath (Join-Path $app $marker) -Raw).Trim() -ne $marker) {
        throw "Refusing to replace an unrelated directory: $app"
    }
}
$shell = New-Object -ComObject WScript.Shell
if (Test-Path -LiteralPath $shortcut) {
    $item = Get-Item -LiteralPath $shortcut -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        $shell.CreateShortcut($shortcut).TargetPath -ne $exe) {
        throw "Refusing to replace an unrelated shortcut: $shortcut"
    }
}
if (Test-Path -LiteralPath "$progKey\shell\open\command") {
    if ((Get-Item -LiteralPath "$progKey\shell\open\command").GetValue('') -ne $command) {
        throw "Refusing to replace an unrelated PDF registration: $progKey"
    }
} elseif (Test-Path -LiteralPath $progKey) {
    throw "Refusing to replace an unrelated PDF registration: $progKey"
}
if ((Test-Path -LiteralPath $openWith) -and
    ((Get-Item -LiteralPath $openWith).GetValueNames() -contains $progId) -and
    -not (Test-Path -LiteralPath $progKey)) {
    throw "Refusing to replace an unrelated PDF registration: $openWith"
}

& cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

New-Item -ItemType Directory -Path $parent -Force | Out-Null
$stage = Join-Path $parent ('.ggPdf-stage-' + [guid]::NewGuid().ToString('N'))
$backup = Join-Path $parent ('.ggPdf-backup-' + [guid]::NewGuid().ToString('N'))
$shortcutBackup = Join-Path $parent ('.ggPdf-shortcut-' + [guid]::NewGuid().ToString('N') + '.lnk')
$placed = $false
$committed = $false
$hadShortcut = Test-Path -LiteralPath $shortcut
$hadProgKey = Test-Path -LiteralPath $progKey
$hadOpenWith = (Test-Path -LiteralPath $openWith) -and
    ((Get-Item -LiteralPath $openWith).GetValueNames() -contains $progId)
try {
    & (Join-Path $root 'scripts/build-windows-app.ps1') -Root $root -OutDir $stage
    Set-Content -LiteralPath (Join-Path $stage $marker) -Value $marker

    if (Test-Path -LiteralPath $app) { Move-Item -LiteralPath $app -Destination $backup }
    Move-Item -LiteralPath $stage -Destination $app
    $placed = $true

    New-Item -ItemType Directory -Path (Split-Path $shortcut) -Force | Out-Null
    if ($hadShortcut) { Copy-Item -LiteralPath $shortcut -Destination $shortcutBackup }
    $link = $shell.CreateShortcut($shortcut)
    $link.TargetPath = $exe
    $link.WorkingDirectory = $app
    $link.IconLocation = "$exe,0"
    $link.Save()

    # Register under Open with, not as the default PDF handler.
    New-Item -Path "$progKey\shell\open\command" -Force | Out-Null
    Set-Item -LiteralPath $progKey -Value 'ggPdf PDF document'
    Set-Item -LiteralPath "$progKey\shell\open\command" -Value $command
    New-Item -Path "$progKey\DefaultIcon" -Force | Out-Null
    Set-Item -LiteralPath "$progKey\DefaultIcon" -Value "$exe,0"
    New-Item -Path $openWith -Force | Out-Null
    New-ItemProperty -LiteralPath $openWith -Name $progId -PropertyType String -Value '' -Force | Out-Null

    $committed = $true
    Write-Host "Installed $app (Start menu: ggPdf; PDF: Open with ggPdf)"
} finally {
    if (-not $committed) {
        if (-not $hadOpenWith -and (Test-Path -LiteralPath $openWith)) {
            Remove-ItemProperty -LiteralPath $openWith -Name $progId -ErrorAction SilentlyContinue
        }
        if (-not $hadProgKey -and (Test-Path -LiteralPath $progKey)) {
            Remove-Item -LiteralPath $progKey -Recurse -Force
        }
        if (-not $hadShortcut -and (Test-Path -LiteralPath $shortcut)) {
            Remove-Item -LiteralPath $shortcut -Force
        }
        if (Test-Path -LiteralPath $shortcutBackup) {
            Copy-Item -LiteralPath $shortcutBackup -Destination $shortcut -Force
        }
        if ($placed) { Remove-Item -LiteralPath $app -Recurse -Force }
        if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $app }
    }
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    if (Test-Path -LiteralPath $shortcutBackup) { Remove-Item -LiteralPath $shortcutBackup -Force }
    if ($committed -and (Test-Path -LiteralPath $backup)) {
        Remove-Item -LiteralPath $backup -Recurse -Force
    }
}

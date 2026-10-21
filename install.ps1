# Register ggpdf in the current user's PDF "Open with" menu; keep the default app.
$ErrorActionPreference = 'Stop'

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq 'Core') {
    throw 'Run this installer in PowerShell on Windows.'
}

$scriptPath = (Resolve-Path (Join-Path $PSScriptRoot 'preview.py')).Path
$python = $null
foreach ($command in @('py', 'python')) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) { continue }
    try {
        $arguments = @()
        if ($command -eq 'py') { $arguments = @('-3') }
        $python = (& $command @arguments -c 'import sys; print(sys.executable)' 2>$null | Select-Object -First 1)
        if ($LASTEXITCODE -eq 0 -and $python -and (Test-Path -LiteralPath $python -PathType Leaf)) { break }
    } catch { }
    $python = $null
}
if (-not $python) { throw 'Python 3 is required. Install Python 3, then rerun this script.' }

$pythonw = Join-Path (Split-Path $python) 'pythonw.exe'
if (Test-Path -LiteralPath $pythonw -PathType Leaf) { $python = $pythonw }

# Create a small ggpdf icon for the file association and app picker.
Add-Type -AssemblyName System.Drawing
$iconPath = Join-Path $PSScriptRoot 'ggpdf.ico'
$bitmap = [System.Drawing.Bitmap]::new(64, 64)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = 'AntiAlias'
$graphics.TextRenderingHint = 'AntiAliasGridFit'
$background = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(40, 40, 40))
$foreground = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::White)
$font = [System.Drawing.Font]::new('Arial', 27, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
try {
    $graphics.FillRectangle($background, 0, 0, 64, 64)
    $graphics.DrawString('gg', $font, $foreground, 7, 12)
    $icon = [System.Drawing.Icon]::FromHandle($bitmap.GetHicon())
    try {
        $stream = [System.IO.File]::Create($iconPath)
        try { $icon.Save($stream) } finally { $stream.Dispose() }
    } finally { $icon.Dispose() }
} finally {
    $font.Dispose()
    $foreground.Dispose()
    $background.Dispose()
    $graphics.Dispose()
    $bitmap.Dispose()
}

$classes = 'Registry::HKEY_CURRENT_USER\Software\Classes'
$progId = 'ggpdf.PDF'
$appKey = Join-Path $classes $progId
$commandKey = Join-Path $appKey 'shell\open\command'
$openWithKey = Join-Path $classes '.pdf\OpenWithProgids'
$capabilities = 'Registry::HKEY_CURRENT_USER\Software\ggpdf\Capabilities'
$registered = 'Registry::HKEY_CURRENT_USER\Software\RegisteredApplications'

New-Item -Path $commandKey -Force | Out-Null
Set-Item -Path $appKey -Value 'ggpdf PDF Preview'
New-ItemProperty -Path $appKey -Name FriendlyTypeName -Value 'ggpdf PDF Preview' -Force | Out-Null
$iconKey = New-Item -Path (Join-Path $appKey 'DefaultIcon') -Force
Set-Item -Path $iconKey.PSPath -Value ('"{0}"' -f $iconPath)
Set-Item -Path $commandKey -Value ('"{0}" "{1}" "%1"' -f $python, $scriptPath)
New-Item -Path $openWithKey -Force | Out-Null
New-ItemProperty -Path $openWithKey -Name $progId -PropertyType String -Value '' -Force | Out-Null

New-Item -Path $capabilities -Force | Out-Null
New-ItemProperty -Path $capabilities -Name ApplicationName -Value 'ggpdf PDF Preview' -Force | Out-Null
New-ItemProperty -Path $capabilities -Name ApplicationDescription -Value 'Keyboard-driven PDF viewer' -Force | Out-Null
New-ItemProperty -Path $capabilities -Name ApplicationIcon -Value ('"{0}"' -f $iconPath) -Force | Out-Null
$associations = New-Item -Path (Join-Path $capabilities 'FileAssociations') -Force
New-ItemProperty -Path $associations.PSPath -Name '.pdf' -Value $progId -Force | Out-Null
New-Item -Path $registered -Force | Out-Null
New-ItemProperty -Path $registered -Name 'ggpdf PDF Preview' -Value 'Software\ggpdf\Capabilities' -Force | Out-Null

if (-not ('ShellRegistration' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ShellRegistration {
    [DllImport("shell32.dll")]
    public static extern void SHChangeNotify(int eventId, uint flags, IntPtr item1, IntPtr item2);
}
'@
}
[ShellRegistration]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host 'Registered ggpdf for PDFs. Right-click a PDF > Open with > Choose another app > ggpdf PDF Preview.'

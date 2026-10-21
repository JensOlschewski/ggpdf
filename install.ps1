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

$classes = 'Registry::HKEY_CURRENT_USER\Software\Classes'
$progId = 'ggpdf.PDF'
$appKey = Join-Path $classes $progId
$commandKey = Join-Path $appKey 'shell\open\command'
$openWithKey = Join-Path $classes '.pdf\OpenWithProgids'

New-Item -Path $commandKey -Force | Out-Null
Set-Item -Path $appKey -Value 'ggpdf PDF Preview'
Set-Item -Path $commandKey -Value ('"{0}" "{1}" "%1"' -f $python, $scriptPath)
New-Item -Path $openWithKey -Force | Out-Null
New-ItemProperty -Path $openWithKey -Name $progId -PropertyType String -Value '' -Force | Out-Null

Write-Host 'Registered ggpdf for PDFs. Right-click a PDF > Open with > Choose another app > ggpdf PDF Preview.'

# Register ggpdf in the current user's PDF "Open with" menu.
# Keep the current default PDF application.
$ErrorActionPreference = 'Stop'

if ($env:OS -ne 'Windows_NT') {
    throw 'Run this installer on Windows.'
}

$scriptPath = (Resolve-Path -LiteralPath (
    Join-Path $PSScriptRoot 'preview.py'
)).Path

# Find a working Python 3 installation.
$python = $null

foreach ($name in @('py', 'python', 'python3')) {
    $cmd = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue
    if (-not $cmd) { continue }

    try {
        $pythonArgs = @(
            '-c'
            'import sys; print(sys.executable); sys.exit(0 if sys.version_info.major == 3 else 1)'
        )

        if ($name -eq 'py') {
            $pythonArgs = @('-3') + $pythonArgs
        }

        $output = & $cmd.Source @pythonArgs
        $exitCode = $LASTEXITCODE

        if ($exitCode -ne 0) {
            Write-Warning "$name exited with code $exitCode."
            continue
        }

        $candidate = $output | Select-Object -Last 1
        if ($candidate) {
            $candidate = $candidate.ToString().Trim()

            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $python = $candidate
                break
            }
        }

        Write-Warning "$name returned an invalid executable path: '$candidate'"
    } catch {
        Write-Warning "Python detection failed for ${name}: $_"
    }
}

if (-not $python) {
    throw 'No working Python 3 installation found. See warnings above.'
}

Write-Host "Found Python: $python"

# Prefer pythonw to avoid opening a console when launching PDFs.
$pythonw = Join-Path (Split-Path -Parent $python) 'pythonw.exe'
if (Test-Path -LiteralPath $pythonw -PathType Leaf) {
    $python = $pythonw
}

$openCommand = '"{0}" "{1}" "%1"' -f $python, $scriptPath
$classesRoot = 'HKCU:\Software\Classes'
$progId = 'ggpdf.PDF'

function Set-RegistryDefault {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Value
    )

    New-Item -Path $Path -Force | Out-Null
    Set-Item -LiteralPath $Path -Value $Value
}

# Register the ggpdf file type.
$progIdPath = Join-Path $classesRoot $progId
Set-RegistryDefault -Path $progIdPath -Value 'ggpdf PDF preview'
Set-RegistryDefault -Path "$progIdPath\DefaultIcon" -Value (
    '"{0}",0' -f $python
)
Set-RegistryDefault -Path "$progIdPath\shell\open\command" -Value $openCommand

# Add ggpdf to the PDF "Open with" list without changing the default app.
$openWithPath = "$classesRoot\.pdf\OpenWithProgids"
New-Item -Path $openWithPath -Force | Out-Null
New-ItemProperty `
    -Path $openWithPath `
    -Name $progId `
    -PropertyType String `
    -Value '' `
    -Force | Out-Null

# Register application capabilities for Windows' app picker.
$capabilitiesPath = 'HKCU:\Software\ggpdf\Capabilities'
New-Item -Path $capabilitiesPath -Force | Out-Null

New-ItemProperty `
    -Path $capabilitiesPath `
    -Name 'ApplicationName' `
    -PropertyType String `
    -Value 'ggpdf' `
    -Force | Out-Null

New-ItemProperty `
    -Path $capabilitiesPath `
    -Name 'ApplicationDescription' `
    -PropertyType String `
    -Value 'PDF preview with ggpdf' `
    -Force | Out-Null

$associationsPath = "$capabilitiesPath\FileAssociations"
New-Item -Path $associationsPath -Force | Out-Null
New-ItemProperty `
    -Path $associationsPath `
    -Name '.pdf' `
    -PropertyType String `
    -Value $progId `
    -Force | Out-Null

$registeredAppsPath = 'HKCU:\Software\RegisteredApplications'
New-Item -Path $registeredAppsPath -Force | Out-Null
New-ItemProperty `
    -Path $registeredAppsPath `
    -Name 'ggpdf' `
    -PropertyType String `
    -Value 'Software\ggpdf\Capabilities' `
    -Force | Out-Null

# Notify Explorer that file associations have changed.
if (-not ('Ggpdf.ShellNotifications' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace Ggpdf
{
    public static class ShellNotifications
    {
        [DllImport("shell32.dll")]
        public static extern void SHChangeNotify(
            uint eventId,
            uint flags,
            IntPtr item1,
            IntPtr item2
        );
    }
}
'@
}

[Ggpdf.ShellNotifications]::SHChangeNotify(
    0x08000000,
    0,
    [IntPtr]::Zero,
    [IntPtr]::Zero
)

Write-Host 'ggpdf registered successfully.'
Write-Host 'Right-click a PDF and select Open with > ggpdf.'
Write-Host 'Your default PDF application has not been changed.'
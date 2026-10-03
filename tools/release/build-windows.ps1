param(
    [switch]$Installer
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$dist = Join-Path $root 'dist\Axiom-1.3.0-windows-x86_64'
Set-Location $root

cargo build --release -p axiom-desktop
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item 'target\release\axiom.exe' (Join-Path $dist 'axiom.exe') -Force
Copy-Item 'LICENSE' (Join-Path $dist 'LICENSE.txt') -Force
Copy-Item 'docs\RELEASES\1.2.8.md' (Join-Path $dist 'RELEASE-NOTES.md') -Force
Compress-Archive -Path "$dist\*" -DestinationPath (Join-Path $root 'dist\Axiom-1.3.0-windows-x86_64.zip') -Force

if ($Installer) {
    $nsis = Get-Command makensis -ErrorAction SilentlyContinue
    if (-not $nsis) { throw 'NSIS is required for -Installer. Install NSIS, then rerun this script.' }
    & $nsis.Source (Join-Path $root 'installer\windows\axiom.nsi')
    Move-Item (Join-Path $root 'Axiom-Setup-1.3.0.exe') (Join-Path $root 'dist\Axiom-Setup-1.3.0.exe') -Force
}

# scripts/install-innosetup.ps1 -- install Inno Setup into <repo>\tools\InnoSetup6
#
# The v2 installer prefers Inno Setup 6.7+ for its dark wizard. This script
# downloads the official installer with GitHub CLI (the direct HTTP path is
# often blocked on this network) and installs it silently into the repo-local
# tools directory. It is optional: build-release-v2.ps1 falls back to NSIS.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\install-innosetup.ps1
#
# Requires an authenticated gh CLI:
#   gh auth login

param(
    [string]$Version = "6.7.3",
    [string]$Dir = ""
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
if (-not $Dir) { $Dir = Join-Path $Root "tools\InnoSetup6" }

$gh = Get-ChildItem "$env:LOCALAPPDATA\Programs\gh" -Recurse -Filter gh.exe -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $gh) {
    $cmd = Get-Command gh -ErrorAction SilentlyContinue
    if ($cmd) { $gh = $cmd.Source }
}
if (-not $gh) { throw "gh.exe not found. Install GitHub CLI and run: gh auth login" }

& $gh auth status *> $null
if ($LASTEXITCODE -ne 0) { throw "gh is not logged in. Run: `"$gh`" auth login" }

$tag = "is-" + ($Version -replace '\.', '_')
$asset = "innosetup-$Version.exe"
$tmp = Join-Path $env:TEMP $asset
Write-Host "Downloading $asset ($tag)..." -ForegroundColor Yellow
& $gh release download $tag -R jrsoftware/issrc -p $asset -D $env:TEMP --clobber
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $tmp)) {
    throw "Download failed. Check gh auth / network, or install Inno Setup manually."
}

Write-Host "Installing to $Dir..." -ForegroundColor Yellow
Start-Process -FilePath $tmp -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR=`"$Dir`"" -Wait

if (-not (Test-Path (Join-Path $Dir "ISCC.exe"))) {
    throw "ISCC.exe not found under $Dir"
}
Remove-Item $tmp -Force -ErrorAction SilentlyContinue
Write-Host "OK: $(Join-Path $Dir 'ISCC.exe')" -ForegroundColor Green

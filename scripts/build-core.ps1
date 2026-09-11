# Build standalone binaries for CLI and Proxy (self-contained, no Bun runtime needed)
# Run from project root
param(
    [switch]$SkipCli
)

$ErrorActionPreference = "Stop"
$coreDir = "$PSScriptRoot\..\core"

Write-Host "=== Building CLI ===" -ForegroundColor Cyan
if (-not $SkipCli) {
    Push-Location $coreDir
    bun build --compile entrypoints/cli.tsx --outfile cli.exe
    Pop-Location
    $cliSize = [math]::Round((Get-Item "$coreDir\cli.exe").Length / 1MB, 1)
    Write-Host "  cli.exe  $cliSize MB" -ForegroundColor Green
}

Write-Host "=== Build complete ===" -ForegroundColor Green

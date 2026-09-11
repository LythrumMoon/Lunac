# Set PATH for cargo and mingw64
$env:PATH = "$env:USERPROFILE\.cargo\bin;D:\cc\claude-code-cli-master\mingw64\bin;$env:PATH"

# Build standalone binaries if missing
$projectRoot = "$PSScriptRoot\.."
if (-not (Test-Path "$projectRoot\core\cli.exe")) {
    Write-Host "Building standalone binaries (one-time)..." -ForegroundColor Yellow
    & "$PSScriptRoot\build-core.ps1"
}

Set-Location "$projectRoot\app"
npm run tauri:dev

# scripts/dev.ps1 — 开发模式；缺 cli.exe 时先尝试用本机 core 源码编译
. "$PSScriptRoot\_env.ps1"

$projectRoot = "$PSScriptRoot\.."
if (-not (Test-Path "$projectRoot\core\cli.exe")) {
    Write-Host "core\cli.exe 缺失，尝试用本机 core 源码编译（一次性）..." -ForegroundColor Yellow
    & "$PSScriptRoot\build-core.ps1"
}

Set-Location "$projectRoot\app"
npm run tauri:dev

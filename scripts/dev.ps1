# scripts/dev.ps1 — 开发模式；缺 agent.exe 时先用 cargo 编译 core-agent
. "$PSScriptRoot\_env.ps1"

$projectRoot = "$PSScriptRoot\.."
if (-not (Test-Path "$projectRoot\core-agent\target\release\agent.exe")) {
    Write-Host "agent.exe 缺失，编译 core-agent（一次性）..." -ForegroundColor Yellow
    & "$PSScriptRoot\build-core.ps1"
}

Set-Location "$projectRoot\app"
npm run tauri:dev

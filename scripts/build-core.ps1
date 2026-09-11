# scripts/build-core.ps1 — 编译自研 agent 后端（core-agent → agent.exe）
# 产物：core-agent\target\release\agent.exe，由 src-tauri 的 core_dir() 查找。
. "$PSScriptRoot\_env.ps1"

$ErrorActionPreference = "Stop"
$coreAgentDir = "$PSScriptRoot\..\core-agent"

Write-Host "=== Building core-agent (agent.exe) ===" -ForegroundColor Cyan
Push-Location $coreAgentDir
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
} finally { Pop-Location }

$exe = "$coreAgentDir\target\release\agent.exe"
if (-not (Test-Path $exe)) { throw "agent.exe not found at $exe" }
Write-Host ("  agent.exe  {0} MB" -f [math]::Round((Get-Item $exe).Length / 1MB, 1)) -ForegroundColor Green

Write-Host "=== Build complete ===" -ForegroundColor Green

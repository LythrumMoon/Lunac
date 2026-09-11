# scripts/tauri-build.ps1 — 构建 Tauri 发行产物（不含 NSIS 安装包）
# 需要 core\cli.exe 已就位（bundle.resources 会引用它）
. "$PSScriptRoot\_env.ps1"

Set-Location "$PSScriptRoot\..\app"
npm run tauri:build

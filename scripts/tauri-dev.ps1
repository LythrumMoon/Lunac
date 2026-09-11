# scripts/tauri-dev.ps1 — 开发模式（Vite 5173 + Tauri 窗口）
. "$PSScriptRoot\_env.ps1"

Set-Location "$PSScriptRoot\..\app"
npm run tauri:dev

# scripts/tauri-build.ps1 — 构建 Tauri 发行产物（不含 NSIS 安装包）
# 需要 agent.exe 已就位（bundle.resources 会引用它）：先跑 scripts\build-core.ps1
. "$PSScriptRoot\_env.ps1"

Set-Location "$PSScriptRoot\..\app"

# ── 构建前清掉上一次的产物（2026-09-19 新增）──────────────────────
# 只删「产物」，**绝不碰编译缓存**（`target\release\{deps,build,incremental}`）——
# 删缓存 = 每次全量重编 549 个 crate，构建从十几分钟退化到更久。
# 为什么必须删：① Tauri 的 NSIS 打包器在输出文件已存在时只会含糊地报
# `Can't open output file`（与 build-release.ps1 第 ⑨ 步同源，杀软正在扫描旧包时
# 尤其容易命中），不删就等着构建跑到最后一步才失败；② 顺带保证拿到的永远是本次
# 编译的 exe，而不是上次残留的。
# `agent.exe` 也在删除之列：它是 `bundle.resources` 从 core-agent 平铺过来的**副本**，
# 构建时会重新拷贝；源文件在 core-agent\target\release 下，不受影响。
$releaseDir = Join-Path $PSScriptRoot "..\app\src-tauri\target\release"
$stale = @(
  (Join-Path $releaseDir "bundle"),     # 上一次的安装包输出目录（nsis/msi）
  (Join-Path $releaseDir "lunac.exe"),  # 上一次的可执行文件
  (Join-Path $releaseDir "agent.exe")   # 平铺副本，构建时重拷
)
foreach ($p in $stale) {
  if (-not (Test-Path -LiteralPath $p)) { continue }
  try {
    Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop
    Write-Host "[clean] 已删除 $p"
  } catch {
    throw "无法删除 $p —— 多半是 Lunac / agent 正在运行（文件被占用）。请先退出 Lunac 再重跑。原始错误：$($_.Exception.Message)"
  }
}

npm run tauri:build

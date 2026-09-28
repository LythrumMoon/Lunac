# 打包「可独立安装的磁盘插件」（2026-09-28）
#
# ⚠️ 本文件必须存成 **UTF-8 with BOM**：Windows PowerShell 5.1 对无 BOM 的 .ps1 按
#    ANSI（本机 GBK）解码 —— 下面的中文会变成乱码，而乱码字节可能吃掉引号，
#    脚本直接 **解析失败**（不是运行报错，是根本进不去）。改完文件请确认 BOM 还在。
#    （同类教训见 ai-spec §11：PS 5.1 的 e2e 脚本里写不了中文模式串。）
#
# 产出：release\plugin-packages\<id>-<version>.zip
#   zip 内形状 = 一个插件目录（lunac-plugin.json + index.js），可直接被
#   「设置 → 插件 → 下载 / 从 URL 安装」那条管线安装（见 app/src-tauri/src/plugin_market.rs）。
#
# 为什么单独立一个脚本、不塞进 build-release.ps1：
#   · 安装包**不再**随包分发这些插件（用户按需从市场装），所以它与「打安装包」是两件事；
#   · 发布插件要**上传**到 GitHub Release 才能被市场下载，那一步是人工的（见脚本末尾提示）。
#
# 用法：powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1
#   -Plugin music      只打某一个
#   -NoBuild           跳过 vite 构建，只用现有 app\plugin-dist\ 重新打包+打 zip
#
# 流程：① vite 库模式构建出 app\plugin-dist\<id>\index.js（见 app\vite.plugins.config.ts）
#       ② 按下面的元数据表生成每个插件的 lunac-plugin.json
#       ③ 压成 zip（用 .NET 的 ZipFile，不依赖 7z / Compress-Archive 的目录层级怪癖）

param(
  [string[]]$Plugin = @(),
  [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$AppDir = "$Root\app"
$DistDir = "$AppDir\plugin-dist"
$OutDir = "$Root\release\plugin-packages"

# ── 插件元数据（清单内容 = 界面能搜到的一切）──────────────────────────
# keywords 是搜索的关键（中文关键词会自动生成拼音 token）；dependencies 见
# ai-spec §3.5「依赖随插件装」——**它才是 release 不再缺依赖的机制**。
# 说明为什么 music 的 dependencies 目前是空数组：
#   librespot 没有官方 Windows 二进制发行（上游只发 GitHub Actions 产物），
#   所以它必须由**我们自己**挂到一个 https 直链上（Lunac 仓库的 Release 资产）。
#   挂好之后在这里补一条 { type = "file"; url = "..."; dest = "bin/librespot.exe"; sha256 = "..." }。
$Plugins = @{
  music = @{
    name         = "音乐"
    description  = "LRCLIB 歌词抓取 + Spotify 播放控制"
    keywords     = @("music", "音乐", "歌词", "music 音乐", "spotify", "lyrics", "lrclib")
    icon         = "🎵"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @()
    dependencies = @()
  }
}

# ── 版本号：与 app\package.json 同源（市场上「更新」按钮靠它判断有没有新版）──
function Get-AppVersion {
  $pkg = Join-Path $AppDir "package.json"
  if (-not (Test-Path $pkg)) { throw "找不到 $pkg" }
  return (Get-Content $pkg -Raw | ConvertFrom-Json).version
}

# ── ① 构建 ─────────────────────────────────────────────────────────
if (-not $NoBuild) {
  Write-Host "[1/3] 构建插件包（vite lib 模式）..." -ForegroundColor Yellow
  Push-Location $AppDir
  try {
    & npx vite build --config vite.plugins.config.ts
    if ($LASTEXITCODE -ne 0) { throw "vite 插件构建失败（exit $LASTEXITCODE）" }
  } finally { Pop-Location }
} else {
  Write-Host "[1/3] 跳过构建（-NoBuild）" -ForegroundColor DarkGray
}

$Version = Get-AppVersion
$Ids = if ($Plugin.Count -gt 0) { $Plugin } else { @($Plugins.Keys) }

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

# ── ② 生成 lunac-plugin.json ───────────────────────────────────────
Write-Host "[2/3] 生成清单..." -ForegroundColor Yellow
foreach ($id in $Ids) {
  if (-not $Plugins.ContainsKey($id)) { throw "元数据表里没有插件 '$id'（见本脚本顶部的 `$Plugins）" }
  $meta = $Plugins[$id]
  $dir = Join-Path $DistDir $id
  $entry = Join-Path $dir "index.js"
  if (-not (Test-Path $entry)) {
    throw "缺少构建产物 $entry —— 先跑一次不带 -NoBuild 的构建，或在 vite.plugins.config.ts 的 pluginEntries 里加上它"
  }
  $manifest = [ordered]@{
    id           = $id
    name         = $meta.name
    description  = $meta.description
    keywords     = $meta.keywords
    icon         = $meta.icon
    version      = $Version
    entry        = "index.js"
    homepage     = $meta.homepage
    permissions  = $meta.permissions
    dependencies = $meta.dependencies
  }
  # UTF-8 无 BOM：Rust 侧读清单时对 BOM 有容错，但**别依赖容错**
  $json = $manifest | ConvertTo-Json -Depth 6
  [IO.File]::WriteAllText((Join-Path $dir "lunac-plugin.json"), $json, (New-Object System.Text.UTF8Encoding($false)))
  Write-Host "  $id  v$Version" -ForegroundColor DarkGray
}

# ── ③ 打 zip ───────────────────────────────────────────────────────
# 用 ZipFile::CreateFromDirectory：包内是**目录内容**（lunac-plugin.json 在 zip 根），
# 这正是 install_from_bytes 认的形状之一。
Write-Host "[3/3] 打包 zip..." -ForegroundColor Yellow
Add-Type -AssemblyName System.IO.Compression.FileSystem
foreach ($id in $Ids) {
  $zip = Join-Path $OutDir "$id-$Version.zip"
  if (Test-Path $zip) { Remove-Item $zip -Force }
  [IO.Compression.ZipFile]::CreateFromDirectory(
    (Join-Path $DistDir $id), $zip,
    [IO.Compression.CompressionLevel]::Optimal, $false
  )
  $kb = [math]::Round((Get-Item $zip).Length / 1KB, 1)
  Write-Host "  $zip  ($kb KB)" -ForegroundColor Green
}

Write-Host ""
Write-Host "下一步：把它发布到**公开**插件仓库（终端用户要能免鉴权直链下载）。" -ForegroundColor Yellow
Write-Host "  powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1"
Write-Host "  · 默认目标仓库：LythrumMoon/lunac-plugins（公开；不存在时先用 gh repo create 建）"
Write-Host "  · 它会把 zip 放进 packages\ 并生成 index.json，然后 commit + push"
Write-Host "  · 宿主常量 INDEX_URL 指向该仓库的 index.json（见 src-tauri/src/plugin_market.rs）"
Write-Host ""
Write-Host "只想本机试装（不进市场）：把 zip 解到 <exe 根>\Modules\<id>\，再在 设置 → 插件 → 重新扫描。" -ForegroundColor DarkGray

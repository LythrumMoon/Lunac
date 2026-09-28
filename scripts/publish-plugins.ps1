# 把插件包发布到**公开**的插件仓库，并更新市场索引（2026-09-28）
#
# ⚠️ 本文件必须存成 **UTF-8 with BOM**（Windows PowerShell 5.1 对无 BOM 的 .ps1 按 ANSI
#    解码，中文会变乱码可能吃掉引号 ⇒ 直接解析失败）。改完文件请确认 BOM 还在。
#
# 为什么需要它：插件包与市场索引必须放在**公开、可直链**的地方 —— 私有仓库的 Release 与
# raw 链接，终端用户下载时要鉴权，等于下不到。所以：
#   · 插件仓库（默认 LythrumMoon/lunac-plugins，公开）里放 packages\<id>-<ver>.zip + index.json
#   · 宿主常量 INDEX_URL 指向它的 index.json（见 src-tauri/src/plugin_market.rs）
#   · 每个包的 url 用 raw.githubusercontent.com 直链（https，无需鉴权）
#
# 前置：① 先跑 scripts\build-plugins.ps1 产出 release\plugin-packages\*.zip
#       ② 本机已 `gh auth login`（用 gh 建仓库 / 探活；推送本身走 git）
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1
#   -Repo LythrumMoon/lunac-plugins   换目标仓库
#   -WorkDir D:\cc\lunac-plugins      换本地克隆位置（缺省用仓库的兄弟目录）
#   -NoPush                           只生成 index.json 与拷贝，不 commit / push（先看一眼）

param(
  [string]$Repo = "LythrumMoon/lunac-plugins",
  [string]$WorkDir = "",
  [switch]$NoPush
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$PkgDir = "$Root\release\plugin-packages"
$DistDir = "$Root\app\plugin-dist"

if (-not $WorkDir) {
  $WorkDir = Join-Path (Split-Path -Parent $Root) (Split-Path -Leaf $Repo)
}

# ── 0. 前置检查 ────────────────────────────────────────────────────
$gh = Get-ChildItem "$env:LOCALAPPDATA\Programs\gh" -Recurse -Filter gh.exe -ErrorAction SilentlyContinue |
  Select-Object -First 1 -ExpandProperty FullName
if (-not $gh) {
  $c = Get-Command gh -ErrorAction SilentlyContinue
  if ($c) { $gh = $c.Source }
}
if (-not $gh) { throw "找不到 gh.exe（见 scripts\build-plugins.ps1 的说明，或自行安装 GitHub CLI）" }

& $gh auth status *> $null
if ($LASTEXITCODE -ne 0) { throw "gh 未登录 —— 先跑：`"$gh`" auth login" }

$zips = @(Get-ChildItem $PkgDir -Filter *.zip -ErrorAction SilentlyContinue)
if ($zips.Count -eq 0) { throw "没有可发布的包（$PkgDir 是空的）—— 先跑 scripts\build-plugins.ps1" }

# ── 1. 克隆 / 更新目标仓库 ─────────────────────────────────────────
if (Test-Path (Join-Path $WorkDir ".git")) {
  Write-Host "[1/4] 更新已有克隆：$WorkDir" -ForegroundColor Yellow
  Push-Location $WorkDir
  try { git pull --ff-only } finally { Pop-Location }
} elseif (Test-Path $WorkDir) {
  throw "$WorkDir 已存在但不是 git 仓库 —— 换个 -WorkDir，或先手工清掉它"
} else {
  Write-Host "[1/4] 克隆 $Repo → $WorkDir" -ForegroundColor Yellow
  git clone "https://github.com/$Repo.git" $WorkDir
  if ($LASTEXITCODE -ne 0) {
    throw "克隆失败。仓库还不存在？用 gh repo create $Repo --public 建一个（脚本不自动建，避免建错地方）"
  }
}

$PkgOut = Join-Path $WorkDir "packages"
New-Item -ItemType Directory -Path $PkgOut -Force | Out-Null

# ── 2. 拷包 ────────────────────────────────────────────────────────
Write-Host "[2/4] 拷贝插件包..." -ForegroundColor Yellow
foreach ($z in $zips) {
  Copy-Item $z.FullName -Destination $PkgOut -Force
  Write-Host "  $($z.Name)" -ForegroundColor DarkGray
}

# ── 3. 生成 index.json ─────────────────────────────────────────────
# 条目字段与 Rust 侧 `PluginIndexEntry` 对齐（id/name/description/version/url/keywords/icon/homepage）。
# **只描述「去哪下」**：真正的元数据（permissions / dependencies）在包自己的 lunac-plugin.json 里，
# 装完以包内清单为准 —— 索引过期也不会导致装出别的东西。
Write-Host "[3/4] 生成市场索引..." -ForegroundColor Yellow
$RawBase = "https://raw.githubusercontent.com/$Repo/main/packages"
$entries = @()
foreach ($z in $zips) {
  # 从构建产物里读清单（app\plugin-dist\<id>\lunac-plugin.json）—— zip 名就是 <id>-<version>
  $id = $z.BaseName -replace '-[\d.]+$', ''
  $manifestPath = Join-Path (Join-Path $DistDir $id) "lunac-plugin.json"
  if (-not (Test-Path $manifestPath)) {
    throw "找不到 $manifestPath —— 插件包与构建产物不一致，先重跑 build-plugins.ps1"
  }
  $m = [IO.File]::ReadAllText($manifestPath, (New-Object System.Text.UTF8Encoding($false))) | ConvertFrom-Json
  $entries += [ordered]@{
    id          = $m.id
    name        = $m.name
    description = $m.description
    version     = $m.version
    url         = "$RawBase/$($z.Name)"
    keywords    = $m.keywords
    icon        = $m.icon
    homepage    = $m.homepage
  }
}
# UTF-8 无 BOM：宿主那边 parse_index 对 BOM 有容错，但别依赖容错
$json = ConvertTo-Json -InputObject @($entries) -Depth 6
[IO.File]::WriteAllText((Join-Path $WorkDir "index.json"), $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "  index.json：$($entries.Count) 条" -ForegroundColor DarkGray

if ($NoPush) {
  Write-Host "[4/4] -NoPush：已生成，未提交。" -ForegroundColor Yellow
  Write-Host "  看一眼：notepad $WorkDir\index.json"
  exit 0
}

# ── 4. 提交 / 推送 ─────────────────────────────────────────────────
Write-Host "[4/4] 提交并推送..." -ForegroundColor Yellow
Push-Location $WorkDir
try {
  git add index.json packages
  # 身份沿用本机 git 配置（克隆过来的仓库会继承全局 user.name/email）；
  # 没配过会在这里失败并提示，比我们瞎设一个身份好。
  git commit -m "plugins: publish $($zips.BaseName -join ', ')"
  if ($LASTEXITCODE -ne 0) { Write-Host "  （没有变更要提交）" -ForegroundColor DarkGray }
  git push
  if ($LASTEXITCODE -ne 0) { throw "推送失败" }
} finally { Pop-Location }

Write-Host ""
Write-Host "发布完成。市场索引：https://raw.githubusercontent.com/$Repo/main/index.json" -ForegroundColor Green
Write-Host "（宿主常量 INDEX_URL 应指向它；改索引后用户端最迟 5 分钟内可见 —— raw 有 CDN 缓存）"

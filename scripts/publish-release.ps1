# 发布安装包 + 生成自更新清单（2026-09-30）
#
# ⚠️ 本文件必须存成 **UTF-8 with BOM**（Windows PowerShell 5.1 对无 BOM 的 .ps1 按 ANSI
#    解码，中文会变乱码、乱码字节可能吃掉引号 ⇒ 脚本直接解析失败）。改完请确认 BOM 还在。
#
# 为什么需要它：应用内自更新（`app/src-tauri/src/updater.rs`）要从**公开**的 GitHub Release
# 拿两样东西，且都必须免鉴权直链：
#   · `Lunac-<ver>-Setup.exe` —— 安装包本体
#   · `latest.json`           —— 版本清单（版本号 / 下载直链 / sha256 / 大小 / 说明）
# 宿主用的是固定入口 `/releases/latest/download/latest.json`，所以这里**只需要上传**，
# 既不用改代码，也不用动分支（raw 那条路有 CDN 缓存，刚发的版本可能读到旧的）。
#
# ⚠️ **不要用 `--prerelease`**：`/releases/latest` 会跳过预发布 ⇒ 打了预发布等于所有人都收不到更新。
#
# 前置：① 先跑 `build-release.ps1` 产出 `release\Lunac-<ver>-Setup.exe`
#       ② 本机已 `gh auth login`（`-StageOnly` 时不需要）
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\publish-release.ps1
#   -Repo <owner/name>   换目标仓库（默认主仓 LythrumMoon/Lunac）
#   -Version 0.9.8       指定版本（缺省读 app\package.json —— 与 build-release.ps1 同一个真相源）
#   -Notes "..."         更新说明；会进 latest.json 的 notes（设置面板上给用户看的就是它）
#   -StageOnly           只生成 release\latest.json，不碰 GitHub（先看一眼，或本机没装 gh）

param(
  [string]$Repo = "LythrumMoon/Lunac",
  [string]$Version = "",
  [string]$Notes = "",
  [switch]$StageOnly
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$ReleaseDir = "$Root\release"

# ── 0. 版本：与 build-release.ps1 同源（app\package.json）────────────
if (-not $Version) {
  $pkg = "$Root\app\package.json"
  if (-not (Test-Path $pkg)) { throw "找不到 $pkg —— 定不了版本号，请用 -Version 显式指定" }
  $Version = (Get-Content $pkg -Raw | ConvertFrom-Json).version
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "版本号必须形如 x.y.z，收到：$Version" }

# ── 1. 找安装包 ────────────────────────────────────────────────────
$SetupName = "Lunac-$Version-Setup.exe"
$SetupPath = Join-Path $ReleaseDir $SetupName
if (-not (Test-Path $SetupPath)) {
  throw "找不到安装包 $SetupPath —— 先跑 build-release.ps1"
}

$Sha256 = (Get-FileHash $SetupPath -Algorithm SHA256).Hash.ToLower()
$Size = (Get-Item $SetupPath).Length
$SizeMb = [math]::Round($Size / 1MB, 1)
Write-Host "[1/3] 安装包：$SetupName（$SizeMb MB）" -ForegroundColor Yellow
Write-Host "      sha256 = $Sha256" -ForegroundColor DarkGray

# ── 2. 生成 latest.json ────────────────────────────────────────────
# 字段与 `updater.rs` 的 `UpdateManifest` 一一对应 —— **改动要两边一起改**。
# 直链必须是 `github.com/<repo>/releases/download/...`（宿主只放行 GitHub 自己的域，
# 见 `updater::validate_manifest`），不要写 `objects.githubusercontent.com`。
if (-not $Notes) { $Notes = "Lunac v$Version" }
$Manifest = [ordered]@{
  version = $Version
  notes   = $Notes
  url     = "https://github.com/$Repo/releases/download/v$Version/$SetupName"
  sha256  = $Sha256
  size    = $Size
}
$ManifestPath = Join-Path $ReleaseDir "latest.json"
# **UTF-8 无 BOM**：宿主侧虽然会剥 BOM（`updater::parse_json_lossy`），但清单是给机器读的，
# 没必要带；深度 3 是为了 notes 里的换行被正常转义，不被压成一行。
$json = $Manifest | ConvertTo-Json -Depth 3
[IO.File]::WriteAllText($ManifestPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "[2/3] 清单：release\latest.json" -ForegroundColor Yellow
Write-Host $json -ForegroundColor DarkGray

if ($StageOnly) {
  Write-Host "[3/3] -StageOnly：不推送。要发布就重跑（去掉 -StageOnly）。" -ForegroundColor Yellow
  exit 0
}

# ── 3. 上传到 Release ──────────────────────────────────────────────
# gh 的定位方式与 publish-plugins.ps1 一致（本机可能装在 %LOCALAPPDATA%\Programs\gh）。
$gh = Get-ChildItem "$env:LOCALAPPDATA\Programs\gh" -Recurse -Filter gh.exe -ErrorAction SilentlyContinue |
  Select-Object -First 1 -ExpandProperty FullName
if (-not $gh) {
  $c = Get-Command gh -ErrorAction SilentlyContinue
  if ($c) { $gh = $c.Source }
}
if (-not $gh) { throw "找不到 gh.exe —— 自行安装 GitHub CLI，或用 -StageOnly 只生成清单" }

& $gh auth status *> $null
if ($LASTEXITCODE -ne 0) { throw "gh 未登录 —— 先跑：`"$gh`" auth login" }

$Tag = "v$Version"
Write-Host "[3/3] 上传到 $Repo 的 Release $Tag ..." -ForegroundColor Yellow

# tag 已存在 ⇒ 覆盖资产（重发同一版本时用）；否则新建 Release。
#
# ⚠️ **不能直接 `& $gh release view … *> $null` 后就查退出码**：release 不存在时 gh 会
# **往 stderr 写一行 `release not found`**，而 PowerShell 5.1 在本脚本的
# `$ErrorActionPreference = "Stop"` 下会把原生命令的 stderr 包成 `NativeCommandError`
# 并**当成终止错误抛出** —— 于是永远走不到下面那句 `if`，第一次发版必挂（本机实测：
# `powershell : gh.exe : release not found`）。临时把 EAP 降回 Continue 只包这一次调用，
# 才能拿到真正的退出码。
$__eap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $gh release view $Tag -R $Repo 2>&1 | Out-Null
$exists = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = $__eap
if ($exists) {
  Write-Host "      Release 已存在 → 覆盖资产" -ForegroundColor DarkGray
  & $gh release upload $Tag $SetupPath $ManifestPath -R $Repo --clobber
  if ($LASTEXITCODE -ne 0) { throw "gh release upload 失败（exit $LASTEXITCODE）" }
} else {
  & $gh release create $Tag $SetupPath $ManifestPath -R $Repo --title "Lunac $Tag" --notes $Notes
  if ($LASTEXITCODE -ne 0) { throw "gh release create 失败（exit $LASTEXITCODE）" }
}

Write-Host ""
Write-Host "发布完成：https://github.com/$Repo/releases/tag/$Tag" -ForegroundColor Green
Write-Host "宿主取的清单入口（固定，不要改）：" -ForegroundColor DarkGray
Write-Host "  https://github.com/$Repo/releases/latest/download/latest.json" -ForegroundColor DarkGray
Write-Host "自检：装一份旧版，然后 设置 → 常规 → 检查更新。" -ForegroundColor DarkGray

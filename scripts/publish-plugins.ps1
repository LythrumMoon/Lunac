# 把插件包发布到**公开**的插件仓库，并更新市场索引（2026-09-28）
#
# ⚠️ 本文件必须存成 **UTF-8 with BOM**（Windows PowerShell 5.1 对无 BOM 的 .ps1 按 ANSI
#    解码，中文会变乱码可能吃掉引号 ⇒ 直接解析失败）。改完文件请确认 BOM 还在。
#
# 为什么需要它：插件包与市场索引必须放在**公开、可直链**的地方 —— 私有仓库的 Release 与
# raw 链接，终端用户下载时要鉴权，等于下不到。所以：
#   · 插件仓库（默认 LythrumMoon/lunac-plugins，公开）里放 packages\<分类>\<id>-<ver>.zip + index.json
#   · 宿主常量 INDEX_URL 指向它的 index.json（见 src-tauri/src/plugin_market.rs）
#   · 每个包的 url 用 raw.githubusercontent.com 直链（https，无需鉴权）
#
# 关于**按分类**（2026-10-13 加）：包落到 packages\<分类>\，index.json 里那条 url 写成
#   .../packages/<分类>/<id>-<版本>.zip。宿主**直接用这条完整 url**去下（见 plugin_market.rs 的
#   parse_index / commands.rs 的 install_plugin_from_url 与前端 settings.ts），**不拼「基址 + 固定
#   路径」** —— 所以放进子目录**不会**影响用户拉取，只要 url 里同步带上分类目录即可（本脚本自动带）。
#   分类由下面的 $Categories 表决定；没登记的 id 落到 other\。
#
# 前置：① 先跑 scripts\build-plugins.ps1 产出 release\plugin-packages\*.zip
#       ② 本机已 `gh auth login`（用 gh 建仓库 / 探活；推送本身走 git）
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1
#   -Repo LythrumMoon/lunac-plugins   换目标仓库
#   -WorkDir D:\cc\lunac-plugins      换本地克隆位置（缺省用仓库的兄弟目录）
#   -NoPush                           只生成 index.json 与拷贝，不 commit / push（先看一眼）
#
# 分类由脚本内的 $Categories 表决定（id → 目录名）；新插件不登记则落到 other\。

param(
  [string]$Repo = "LythrumMoon/lunac-plugins",
  [string]$WorkDir = "",
  [switch]$NoPush
)

$ErrorActionPreference = "Stop"

# ── BOM 自愈守卫（2026-10-04，见 ai-spec §11 规则 82）──────────────────
# 开头多一份 BOM 时 PS 5.1 只剥第一份，剩下的 U+FEFF 把首行 `# 注释` 顶成一条命令
# ⇒ `param()` 不再是脚本首语句 ⇒ **参数静默不绑定**（`-Repo` / `-WorkDir` / `-NoPush`
# 全读成空值，表现是「只想看预览、结果直接推了」或是把包发去默认仓库）。
# 这种文件**仍能跑到这里**（首行那个错误是非终止的），所以守卫跑得到：
# 就地把自己修回一份 BOM，然后中止 —— 参数绑定只有**重跑**才能恢复。
$selfPath = $PSCommandPath
$selfBytes = [IO.File]::ReadAllBytes($selfPath)
$bomN = 0
while ($bomN * 3 + 2 -lt $selfBytes.Length -and
       $selfBytes[$bomN * 3] -eq 0xEF -and $selfBytes[$bomN * 3 + 1] -eq 0xBB -and $selfBytes[$bomN * 3 + 2] -eq 0xBF) { $bomN++ }
if ($bomN -gt 1) {
  $fixed = New-Object byte[] ($selfBytes.Length - $bomN * 3 + 3)
  [Array]::Copy([byte[]](0xEF, 0xBB, 0xBF), 0, $fixed, 0, 3)
  [Array]::Copy($selfBytes, $bomN * 3, $fixed, 3, $selfBytes.Length - $bomN * 3)
  [IO.File]::WriteAllBytes($selfPath, $fixed)
  throw "publish-plugins.ps1 开头有 $bomN 份 BOM（正确是 1 份）—— 已自动修正，请**重新运行**一次（命令行参数要靠重跑才恢复绑定）。"
}

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$PkgDir = "$Root\release\plugin-packages"
$DistDir = "$Root\app\plugin-dist"

# ── 分类表（2026-10-13）：插件 id → packages\<分类>\ 的目录名 ──────────────
# 只影响**包里放哪、url 长什么样、README 里怎么归类**，不改变安装 / 升级语义
# （宿主只认 index.json 里那条完整 url）。**新增插件必须在这里登记**，否则落到 other\。
# 现有的 7 个官方插件（music / clipboard-history / ocr / convert / proxy / pet / local-model）
# 全覆盖；与 lunac-plugins 仓库 README 的「分类」一节保持一致。
$Categories = @{
  "convert"           = "productivity"   # 文件转换
  "clipboard-history" = "productivity"   # 剪贴板历史
  "music"             = "media"          # 音乐
  "ocr"               = "ocr"            # 文字识别
  "pet"               = "pet"            # 桌宠
  "local-model"       = "ai"             # 本地决策模型
  "proxy"             = "system"         # 系统代理
}
function Get-PluginCategory([string]$id) {
  if ($Categories.ContainsKey($id)) { return $Categories[$id] }
  Write-Host "  [!] 插件 '$id' 没登记分类 —— 落到 other\" -ForegroundColor Yellow
  return "other"
}

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

# 每个插件**只发最高版本的那个包**（2026-10-01 补）：`build-plugins.ps1` 不删旧 zip，
# 于是换版本后 `plugin-packages\` 里会同时躺着 pet-0.9.8 与 pet-0.9.9。全部拷过去会让
# `index.json` 出现同一个 id 的两条 —— 用户的市场列表里就是两个「桌宠」，一个有更新一个没有。
# 判据是**文件名里的版本**（与 build-plugins.ps1 的 `<id>-<x.y.z>.zip` 命名约定绑定）。
$zips = @($zips |
  Group-Object { $_.BaseName -replace '-[\d.]+$', '' } |
  ForEach-Object {
    $_.Group | Sort-Object { [version]($_.BaseName -replace '^.*-', '') } -Descending | Select-Object -First 1
  })
$stale = @(Get-ChildItem $PkgDir -Filter *.zip -ErrorAction SilentlyContinue) |
  Where-Object { $zips.Name -notcontains $_.Name }
foreach ($s in $stale) {
  Write-Host "  跳过旧包 $($s.Name)（已有更新的版本）" -ForegroundColor DarkGray
}

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

# ── 2. 拷包（按分类进子目录 packages\<分类>\）──────────────────────────
Write-Host "[2/4] 拷贝插件包..." -ForegroundColor Yellow
$keptNames = @{}
foreach ($z in $zips) {
  $id = $z.BaseName -replace '-[\d.]+$', ''
  $cat = Get-PluginCategory $id
  $destDir = Join-Path $PkgOut $cat
  New-Item -ItemType Directory -Path $destDir -Force | Out-Null
  Copy-Item $z.FullName -Destination $destDir -Force
  $keptNames[$z.Name] = $true
  Write-Host "  $cat\$($z.Name)" -ForegroundColor DarkGray
}
# **只留最新版**（2026-10-13 强化）：清掉 packages\ 下所有**不在本次发布集合里**的 zip —— 覆盖两种残留：
#   ① 同一个 id 的旧版本（如 pet-0.9.8）；② 早先平铺在 packages\ 根、如今已改到子目录的旧包。
#   删完靠下面的 `git add packages` 一并登记删除（git ≥ 2.0 的 `git add <pathspec>` 会暂存删除）。
$orphans = @(Get-ChildItem $PkgOut -Recurse -File -Filter *.zip -ErrorAction SilentlyContinue) |
  Where-Object { -not $keptNames.ContainsKey($_.Name) }
foreach ($o in $orphans) {
  Write-Host "  删除旧包 $($o.FullName.Substring($PkgOut.Length + 1))（已被新版本 / 新布局取代）" -ForegroundColor DarkGray
  Remove-Item $o.FullName -Force
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
  $cat = Get-PluginCategory $id
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
    url         = "$RawBase/$cat/$($z.Name)"
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

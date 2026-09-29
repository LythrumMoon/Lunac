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
#
# music 的 librespot（2026-09-28 挂上）：**上游不发 Windows 二进制**
#   （v0.8.0 / v0.7.x 三个 release 的 assets 全是空数组，别再去上游找），
#   所以那份 exe 是我们自己构建的，产物挂在**公开**插件仓库的 Release 资产上
#   （终端用户要能免鉴权直链下载）。复现 / 换版本时照做：
#     cargo install librespot --version 0.8.0 --locked --root release\deps\librespot
#       （0.8.0 在 Windows 上不需要 OpenSSL / Bonjour / protoc，见 ai-spec §4.6）
#     Get-FileHash release\deps\librespot\bin\librespot.exe -Algorithm SHA256
#     gh release create librespot-0.8.0 <exe> -R LythrumMoon/lunac-plugins
#   ⚠️ 下面的 sha256 必须与**上传的那一份**逐字节一致 —— 对不上时宿主会
#      整包拒绝安装（这是有意的：宁可不装，也不装来路不明的二进制）。
$Plugins = @{
  music = @{
    name         = "音乐"
    description  = "LRCLIB 歌词抓取 + Spotify 播放控制"
    keywords     = @("music", "音乐", "歌词", "music 音乐", "spotify", "lyrics", "lrclib")
    icon         = "🎵"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    # `window.float`（2026-09-29）：本插件一律在**悬浮窗**里打开。
    # 宿主不再按 id 认「音乐要开窗」—— 拓展插件要能独立打包分发，宿主写死的 id 表在磁盘插件上查不到。
    permissions  = @("window.float")
    # dest 落在插件目录内 ⇒ `<exe 根>\Modules\music\bin\librespot.exe`；
    # 宿主 music.rs 的 find_librespot() 认这条路径，用户不必自己去填路径。
    dependencies = @(
      @{
        type   = "file"
        url    = "https://github.com/LythrumMoon/lunac-plugins/releases/download/librespot-0.8.0/librespot.exe"
        dest   = "bin/librespot.exe"
        sha256 = "7509c74b1be2bdcd8debcf6575e557f31db80ea0512f157872429b11e92a4c1a"
      }
    )
  }
  # 剪贴板历史（2026-09-29 从 bundle 摘出）：无权限、无依赖 —— 纯前端 + 宿主存储命令。
  # 「复制时顺手记一条」由宿主命令 append_clipboard_entry 负责（主窗口行为，插件没装也要工作）。
  "clipboard-history" = @{
    name         = "剪贴板历史"
    description  = "剪贴板历史管理 —— 复制自动记录"
    keywords     = @("clipboard", "history", "paste", "剪切板", "历史", "剪贴板", "粘贴", "复制")
    icon         = "📋"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @()
    dependencies = @()
  }
  # OCR（2026-09-29 从 bundle 摘出）：`layout.takeover` = 接管整个窗口（双栏：左图右文），
  # 宿主在 main.ts 的 isTakeoverPlugin() 里认这条声明，不再按 id 写死。
  # 引擎（PaddleOCR-json，约 300MB）**不作为依赖**：它装在 <exe 根>\paddle-ocr、由宿主命令
  # `ocr_engine_install` 按需下载（设置 → OCR 引擎 / 插件内的「下载并安装」都能触发）。
  ocr = @{
    name         = "OCR 文字识别"
    description  = "OCR 图片文字识别 (PaddleOCR · 离线高精度)"
    keywords     = @("ocr", "识别", "文字识别", "图像识别", "图片转文字", "截图识别", "图识字", "文字提取")
    icon         = "🔍"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @("layout.takeover")
    dependencies = @()
  }
  # 文件转换（2026-09-29 从 bundle 摘出）：引擎是本机 ffmpeg（宿主的 convert.rs 自己找），
  # 所以同样无依赖。
  convert = @{
    name         = "文件转换"
    description  = "图片 / 音频 / 视频格式互转 (Image / Audio / Video converter · ffmpeg)"
    keywords     = @("转换", "格式转换", "转格式", "convert", "格式", "转码", "提取音频", "图片转换", "视频转换", "音频转换")
    icon         = "🔄"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @()
    dependencies = @()
  }
  # 桌宠（L1，2026-09-29）：**第一个用 `window` 段声明自己形态的插件**。
  # 无权限（`permissions` 是空的）：它不是「一律开悬浮窗」那一类 —— 搜索打开的是
  # **控制台**面板，桌宠窗由控制台里的「显示桌宠」按钮开（理由见 pet.ts 文件头：
  # 穿透开着时桌宠窗收不到鼠标事件，开关必须落在另一个窗口里）。
  pet = @{
    name         = "桌宠"
    description  = "桌宠 —— 独立透明置顶的桌面形象窗（形象由你自己导入）"
    keywords     = @("桌宠", "宠物", "桌面宠物", "pet", "desktop pet", "live2d", "看板娘", "吉祥物")
    icon         = "🐾"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @()
    dependencies = @()
    # `chrome = $false` ⇒ 前端收起标题栏、结果区去玻璃底（styles.css 的 `html.no-chrome`）；
    # `skipTaskbar = $true` ⇒ 不进任务栏（桌宠在任务栏里出现一个条目是纯噪音）；
    # `resizable = $false` ⇒ 禁手动缩放（尺寸由形象与控制台决定）。
    # 三个缺省值反过来的写法（不写这一段）就是历史上所有插件的样子 —— 别把这一段复制给它们。
    window       = @{
      width       = 300
      height      = 400
      minWidth    = 160
      minHeight   = 200
      resizable   = $false
      skipTaskbar = $true
      chrome      = $false
    }
  }
}

# ── 版本号：与 app\package.json 同源（市场上「更新」按钮靠它判断有没有新版）──
function Get-AppVersion {
  $pkg = Join-Path $AppDir "package.json"
  if (-not (Test-Path $pkg)) { throw "找不到 $pkg" }
  return (Get-Content $pkg -Raw | ConvertFrom-Json).version
}

# ── ① 构建 ─────────────────────────────────────────────────────────
# **一个入口一次 vite build**（2026-09-29）：多个入口放一起会让 Rollup 把共享模块提到
# `plugin-dist/<chunk 名>/` 这种跨插件的公共 chunk 里，而插件包只搬走自己那个目录 ⇒
# 那份 chunk 丢失、插件打开即失败。逐个入口各起一次就没有共享 chunk，产物单文件自包含。
$Version = Get-AppVersion
$Ids = if ($Plugin.Count -gt 0) { $Plugin } else { @($Plugins.Keys) }
foreach ($id in $Ids) {
  if (-not $Plugins.ContainsKey($id)) { throw "元数据表里没有插件 '$id'（见本脚本顶部的 `$Plugins）" }
}

if (-not $NoBuild) {
  Write-Host "[1/3] 构建插件包（vite lib 模式，逐个入口）..." -ForegroundColor Yellow
  Push-Location $AppDir
  try {
    foreach ($id in $Ids) {
      $env:LUNAC_PLUGIN = $id
      & npx vite build --config vite.plugins.config.ts
      if ($LASTEXITCODE -ne 0) { throw "vite 插件构建失败（$id，exit $LASTEXITCODE）" }
    }
  } finally {
    Remove-Item Env:\LUNAC_PLUGIN -ErrorAction SilentlyContinue
    Pop-Location
  }
} else {
  Write-Host "[1/3] 跳过构建（-NoBuild）" -ForegroundColor DarkGray
}

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
  # `window` 段只有声明了形态的插件才写（见 plugin_market::PluginWindowShape）。
  # 不写 = 全部走宿主缺省（420×560 / 可缩放 / 进任务栏 / 有标题栏），
  # 也就是这一段加进来之前所有插件的样子 —— **别给它们补一个空对象**，
  # 空对象与不写在这里是等价的，写了只会让人以为是必需的。
  if ($meta.window) { $manifest.window = $meta.window }
  # UTF-8 无 BOM：Rust 侧读清单时对 BOM 有容错，但**别依赖容错**
  $json = $manifest | ConvertTo-Json -Depth 6
  [IO.File]::WriteAllText((Join-Path $dir "lunac-plugin.json"), $json, (New-Object System.Text.UTF8Encoding($false)))
  Write-Host "  $id  v$Version" -ForegroundColor DarkGray
}

# ── ③ 打 zip ───────────────────────────────────────────────────────
# 用 ZipFile::CreateFromDirectory：包内是**目录内容**（lunac-plugin.json 在 zip 根），
# 这正是 install_from_bytes 认的形状之一。
Write-Host "[3/4] 打包 zip..." -ForegroundColor Yellow
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

# ── ④ 给安装包暂存一份「解开的插件目录」─────────────────────────────
# 用途：`scripts\lunac-installer.nsi` 的「拓展插件」勾选段直接从 release\ext-plugins\<id>\
# 拷进 `$INSTDIR\Modules\<id>\`（用户要求：拓展插件不默认装，但安装包里要有勾选项）。
# 用**解开**的目录而不是 zip：安装器没法解压；而且这份形状与「市场装完」的盘上形状一致。
$StageDir = "$Root\release\ext-plugins"
Write-Host "[4/4] 暂存给安装包（release\ext-plugins）..." -ForegroundColor Yellow
foreach ($id in $Ids) {
  $src = Join-Path $DistDir $id
  $dst = Join-Path $StageDir $id
  if (Test-Path $dst) { Remove-Item $dst -Recurse -Force }
  New-Item -ItemType Directory -Path $dst -Force | Out-Null
  Copy-Item -Path (Join-Path $src "*") -Destination $dst -Recurse -Force
  Write-Host "  release\ext-plugins\$id" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "下一步：把它发布到**公开**插件仓库（终端用户要能免鉴权直链下载）。" -ForegroundColor Yellow
Write-Host "  powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1"
Write-Host "  · 默认目标仓库：LythrumMoon/lunac-plugins（公开；不存在时先用 gh repo create 建）"
Write-Host "  · 它会把 zip 放进 packages\ 并生成 index.json，然后 commit + push"
Write-Host "  · 宿主常量 INDEX_URL 指向该仓库的 index.json（见 src-tauri/src/plugin_market.rs）"
Write-Host ""
Write-Host "只想本机试装（不进市场）：把 zip 解到 <exe 根>\Modules\<id>\，再在 设置 → 插件 → 重新扫描。" -ForegroundColor DarkGray

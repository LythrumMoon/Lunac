# 打包「可独立安装的磁盘插件」（2026-09-28）
#
# ⚠️ 本文件必须存成 **UTF-8 with BOM**：Windows PowerShell 5.1 对无 BOM 的 .ps1 按
#    ANSI（本机 GBK）解码 —— 下面的中文会变成乱码，而乱码字节可能吃掉引号，
#    脚本直接 **解析失败**（不是运行报错，是根本进不去）。改完文件请确认 BOM 还在。
#    （同类教训见 ai-spec §11：PS 5.1 的 e2e 脚本里写不了中文模式串。）
#
# 产出：release\plugin-packages\<id>-<插件自己的版本>.zip
#   ⚠️ 版本号**不跟应用版本**（2026-10-01 改）：见下面 `$Plugins` 表与 Get-PluginVersion。
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
# 流程：① vite 库模式构建出 app\plugin-dist\<id>\index.js（见 app/vite.plugins.config.ts）
#       ② 按下面的元数据表生成每个插件的 lunac-plugin.json
#       ③ 压成 zip（用 .NET 的 ZipFile，不依赖 7z / Compress-Archive 的目录层级怪癖）

param(
  [string[]]$Plugin = @(),
  [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

# ── BOM 自愈守卫（2026-10-04，见 ai-spec §11 规则 82）──────────────────
# 开头多一份 BOM 时 PS 5.1 只剥第一份，剩下的 U+FEFF 把首行 `# 注释` 顶成一条命令
# ⇒ `param()` 不再是脚本首语句 ⇒ **参数静默不绑定**（`-Plugin` / `-NoBuild` 全读成空值，
# 表现是「只想打一个插件、结果全量重建」或是下面那句「元数据表里没有插件 ''」）。
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
  throw "build-plugins.ps1 开头有 $bomN 份 BOM（正确是 1 份）—— 已自动修正，请**重新运行**一次（命令行参数要靠重跑才恢复绑定）。"
}

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
# `version` = **本插件自己的版本号**，与应用版本各走各的（2026-10-01 用户要求，
# 见下面 Get-PluginVersion 那一段的完整理由）。**只有真的改了这个插件，才动它这一行。**
$Plugins = @{
  music = @{
    version      = "0.9.27"
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
    version      = "0.9.8"
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
  # 引擎（PaddleOCR-json，压缩后约 88MB / 解压约 300MB）**作为 `archive` 依赖**（2026-09-30 改）：
  #   以前它随安装包分发、缺失时由 `ocr_engine_install` 自下载；现在**两处都不做** ——
  #   装插件时由宿主按这条依赖下载解压到 `<exe 根>\Modules\ocr\paddle-ocr\`，
  #   卸载插件 = 引擎一起清掉（见 ai-spec §3.5 与 plugin_market.rs 的 install_archive_dependency）。
  #   于是「不为少数人的功能让所有用户多下 70MB」与「引擎的获取只有一条路」同时成立。
  #   `dest` 必须落在插件目录内；`sha256` 是**压缩包本身**的校验和（不是解压产物），
  #   对不上时宿主整包拒绝安装 —— 换版本时务必同步更新（下载后 Get-FileHash -Algorithm SHA256）。
  #   上游 Releases 直链（公开、免鉴权）；该资产只有 `.7z`（没有 Windows 的 zip）。
  ocr = @{
    version      = "0.9.10"
    name         = "OCR 文字识别"
    description  = "OCR 图片文字识别 (PaddleOCR · 离线高精度)"
    keywords     = @("ocr", "识别", "文字识别", "图像识别", "图片转文字", "截图识别", "图识字", "文字提取")
    icon         = "🔍"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @("layout.takeover")
    dependencies = @(
      @{
        type   = "archive"
        url    = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/v1.4.1/PaddleOCR-json_v1.4.1_windows_x64.7z"
        dest   = "paddle-ocr"
        sha256 = "C0912A70ACB1F8F18FAFE1F438A2935292A6EC7E2859156FA48A33E91358D71D"
      }
    )
  }
  # 文件转换（2026-09-29 从 bundle 摘出）：引擎是本机 ffmpeg（宿主的 convert.rs 自己找），
  # 所以同样无依赖。
  convert = @{
    version      = "0.9.8"
    name         = "文件转换"
    description  = "图片 / 音频 / 视频格式互转 (Image / Audio / Video converter · ffmpeg)"
    keywords     = @("转换", "格式转换", "转格式", "convert", "格式", "转码", "提取音频", "图片转换", "视频转换", "音频转换")
    icon         = "🔄"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @()
    dependencies = @()
  }
  # 代理（2026-10-02）：系统代理增删启用 + 让本机播放（librespot）走代理。
  # **零依赖**：宿主那半边 2026-10-01 就写好了（`src-tauri/src/system_proxy.rs` 的读/写/
  # 备份/退出还原 + 5 条命令），本插件只是补上**界面**；librespot 的代理走宿主新命令
  # `librespot_set_proxy`（代理是启动参数，改了要重起进程）。
  # permissions 只有 `layout.takeover`（接管型面板，与 OCR 同一条路）。
  proxy = @{
    version      = "0.9.1"
    name         = "代理"
    description  = "系统代理管理 + 让本机播放（librespot）走代理"
    keywords     = @("代理", "proxy", "系统代理", "梯子", "vpn", "socks5", "http proxy", "clash")
    icon         = "🌐"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @("layout.takeover")
    dependencies = @()
  }
  # 桌宠（L1，2026-09-29）：**第一个用 `window` 段声明自己形态的插件**。
  # 无权限（`permissions` 是空的）：它不是「一律开悬浮窗」那一类 —— 搜索打开的是
  # **控制台**面板，桌宠窗由控制台里的「显示桌宠」按钮开（理由见 pet.ts 文件头：
  # 穿透开着时桌宠窗收不到鼠标事件，开关必须落在另一个窗口里）。
  pet = @{
    version      = "0.9.9"
    name         = "桌宠"
    description  = "桌宠 —— 独立透明置顶的桌面形象窗（图片或 Live2D 模型，由你自己导入）"
    keywords     = @("桌宠", "宠物", "桌面宠物", "pet", "desktop pet", "live2d", "看板娘", "吉祥物")
    icon         = "🐾"
    homepage     = "https://github.com/LythrumMoon/Lunac"
    permissions  = @()
    # Live2D 的 **Cubism Core**（专有文件；协议 5.1 允许随包复制转发，义务见
    # docs/agent-feature-backlog.md 的 L1-B ① —— 随附 LICENSE 已落地，见下面的 `notices`）。
    # **为什么走 dependencies 而不是打进 index.js**（2026-09-30 三条本机实测的结论，见 L1 ②）：
    #   ① 官方 URL `cubism.live2d.com/.../live2dcubismcore.min.js` **不带版本号** ⇒ 把它的
    #      sha256 钉死在清单里，会被上游某次更新**卡死安装**；而且那条链路**连接极不稳定**，
    #      当运行时路径不可靠。
    #   ② npm 上的 `live2dcubismcore` 是**违规转载包**（56MB，除专有 Core 外还塞进官方 Haru
    #      样例模型却标 `license: ISC`）—— 正好踩 L1-B ② 的 No Redistribution，绝不进依赖树。
    #   ⇒ 自己托管一份**带版本 + sha256** 的副本，挂在**公开**插件仓库的 Release 上
    #      （与 music 的 librespot 同一条路；终端用户要能免鉴权直链下载）。
    # 换版本时：取一份新的官方 Core → `Get-FileHash -Algorithm SHA256` → 改下面的 url/sha256
    #      与版本标签，同时更新 `local` 指向的本机副本。
    # `local` = **打包机上的本地副本**（相对 `release\`），只用于给安装包暂存（见第 ④ 步）——
    #      它**不会**进清单（写清单时会摘掉），安装器不会自己下载依赖。
    dependencies = @(
      @{
        type   = "file"
        url    = "https://github.com/LythrumMoon/lunac-plugins/releases/download/live2dcubismcore-5.1.0/live2dcubismcore.min.js"
        dest   = "engine/live2dcubismcore.min.js"
        sha256 = "25ae938cb4fe282ce189b357bcc97e603d1e1f7ec78bf04150d401c23cdc792f"
        local  = "deps\live2d\live2dcubismcore.min.js"
        # **协议 5.1 的随附义务**（2026-10-01 补）：官方 `RedistributableFiles.txt` 把
        #   `live2dcubismcore.min.js` 列为可再分发代码，代价是**必须保留随附的 LICENSE 与
        #   RedistributableFiles**。两份原文放在仓库的 `licenses\live2d-cubism-core\`，
        #   打包时复制到 Core 旁边（布局与官方 SDK 的 `Core/` 一致）；**缺一份就打包失败**，
        #   见第 ② 步 —— 宁可不打包，也不发一个没有授权文件的包。
        #   出处：官方 `CubismSdkForWeb-5-r.1.zip` 的 `Core/`（与 5-r.5 逐字节相同 ⇒
        #   这两份与 Core 版本无关）；`LICENSE.md` sha256 `B81E37048010EF…`、
        #   `RedistributableFiles.txt` sha256 `D16C123688…`。换 Core 版本时重取一次比对即可。
        notices = @(
          @{ src = "licenses\live2d-cubism-core\LICENSE.md";               dest = "engine/LICENSE.md" },
          @{ src = "licenses\live2d-cubism-core\RedistributableFiles.txt"; dest = "engine/RedistributableFiles.txt" }
        )
      }
    )
    # **附加入口**：`<插件目录内的相对路径> = <源码路径>`。
    # 桌宠的 Live2D 引擎（pixi + pixi-live2d-display）必须在 **Cubism Core 之后**才敢求值 —— 那个库
    # 在模块求值时就 `if (!window.Live2DCubismCore) throw` —— 而 `inlineDynamicImports` 会把
    # 「同包内惰性 import」提前到顶层求值 ⇒ 只能单打成一个独立 ESM，运行时按绝对 URL 动态 import。
    # 完整理由见 app/vite.plugins.config.ts 的「附加入口」那一段与 src/plugins/builtin/live2d.ts 文件头。
    extraEntries = [ordered]@{ "engine/live2d-engine.js" = "src/plugins/builtin/live2d-engine.ts" }
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

# ── 版本号：**每个插件各带一份，不跟随应用版本**（2026-10-01 改，用户要求）──
# 判据在用户那一侧：市场列表把「索引版本 !== 已装版本」直接叫**更新**
# （settings.ts 的 mergeMarketRows，只判不同、不判大小）。所以版本号一旦与应用版本同源，
# 每次应用发版都会把 5 个插件的版本一起推上去 ⇒ **内容一个字节都没变的插件也被判成「有更新」**，
# 用户被反复喊去更新。改成各带各的以后：**只有真的改了这个插件，才手动动它那一个 `version`**。
#
# ⚠️ 两条配套纪律：
#   · `i18n.ts` / `styles.css` 是**所有插件共用**的输入（会被内联进每个 index.js）——
#     给 A 插件加一句话会让 5 个包的**字节**都变，但其余 4 个的**行为**没变 ⇒
#     只有 A 需要 +1（别按内容哈希自动推版本，那正是「没变化也喊更新」）。
#   · 改完插件源码**必须跑一次本脚本**才生效（盘上是 `Modules\<id>\index.js`，不是源码）。
function Get-PluginVersion([string]$id) {
  $v = $Plugins[$id].version
  if (-not $v) { throw "插件 '$id' 没写 version（见本脚本顶部的 `$Plugins 表）" }
  if ($v -notmatch '^\d+\.\d+\.\d+$') { throw "插件 '$id' 的 version 必须形如 x.y.z，收到：$v" }
  return $v
}

# ── ① 构建 ─────────────────────────────────────────────────────────
# **一个入口一次 vite build**（2026-09-29）：多个入口放一起会让 Rollup 把共享模块提到
# `plugin-dist/<chunk 名>/` 这种跨插件的公共 chunk 里，而插件包只搬走自己那个目录 ⇒
# 那份 chunk 丢失、插件打开即失败。逐个入口各起一次就没有共享 chunk，产物单文件自包含。
$Ids = if ($Plugin.Count -gt 0) { $Plugin } else { @($Plugins.Keys) }
foreach ($id in $Ids) {
  if (-not $Plugins.ContainsKey($id)) { throw "元数据表里没有插件 '$id'（见本脚本顶部的 `$Plugins）" }
}

if (-not $NoBuild) {
  Write-Host "[1/5] 构建插件包（vite lib 模式，逐个入口）..." -ForegroundColor Yellow
  Push-Location $AppDir
  try {
    foreach ($id in $Ids) {
      $env:LUNAC_PLUGIN = $id
      & npx vite build --config vite.plugins.config.ts
      if ($LASTEXITCODE -ne 0) { throw "vite 插件构建失败（$id，exit $LASTEXITCODE）" }
      # 附加入口：同一个插件**再起一次**构建，产物落在插件目录内的子路径下
      #（为什么必须是独立文件、不能再多入口里一次打完，见 vite.plugins.config.ts 那一段）
      $extras = $Plugins[$id].extraEntries
      if ($extras) {
        foreach ($out in $extras.Keys) {
          $env:LUNAC_PLUGIN_EXTRA_OUT = $out
          $env:LUNAC_PLUGIN_EXTRA_SRC = $extras[$out]
          & npx vite build --config vite.plugins.config.ts
          if ($LASTEXITCODE -ne 0) { throw "vite 附加入口构建失败（$id / $out，exit $LASTEXITCODE）" }
        }
        Remove-Item Env:\LUNAC_PLUGIN_EXTRA_OUT -ErrorAction SilentlyContinue
        Remove-Item Env:\LUNAC_PLUGIN_EXTRA_SRC -ErrorAction SilentlyContinue
      }
    }
  } finally {
    Remove-Item Env:\LUNAC_PLUGIN -ErrorAction SilentlyContinue
    Remove-Item Env:\LUNAC_PLUGIN_EXTRA_OUT -ErrorAction SilentlyContinue
    Remove-Item Env:\LUNAC_PLUGIN_EXTRA_SRC -ErrorAction SilentlyContinue
    Pop-Location
  }
} else {
  Write-Host "[1/5] 跳过构建（-NoBuild）" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

# ── ② 生成 lunac-plugin.json ───────────────────────────────────────
Write-Host "[2/5] 生成清单..." -ForegroundColor Yellow
foreach ($id in $Ids) {
  if (-not $Plugins.ContainsKey($id)) { throw "元数据表里没有插件 '$id'（见本脚本顶部的 `$Plugins）" }
  $meta = $Plugins[$id]
  # 版本号**逐插件取**（不是应用版本）：清单里的 version 就是市场判「有没有更新」的判据，
  # 见 Get-PluginVersion 那一段。
  $pv = Get-PluginVersion $id
  $dir = Join-Path $DistDir $id
  $entry = Join-Path $dir "index.js"
  if (-not (Test-Path $entry)) {
    throw "缺少构建产物 $entry —— 先跑一次不带 -NoBuild 的构建，或在 vite.plugins.config.ts 的 pluginEntries 里加上它"
  }
  # `local` / `notices` 都是**打包机专用**的字段（本机副本在哪、随附文件从哪拷）—— 它们都
  # 不属于插件包的契约，写清单时必须摘掉：留在里面就等于往用户机器上多塞没人认的字段。
  $deps = @()
  foreach ($d in $meta.dependencies) {
    $clean = [ordered]@{}
    foreach ($k in $d.Keys) {
      if ($k -ne "local" -and $k -ne "notices") { $clean[$k] = $d[$k] }
    }
    $deps += $clean
  }

  $manifest = [ordered]@{
    id           = $id
    name         = $meta.name
    description  = $meta.description
    keywords     = $meta.keywords
    icon         = $meta.icon
    version      = $pv
    entry        = "index.js"
    homepage     = $meta.homepage
    permissions  = $meta.permissions
    dependencies = $deps
  }
  # `window` 段只有声明了形态的插件才写（见 plugin_market::PluginWindowShape）。
  # 不写 = 全部走宿主缺省（420×560 / 可缩放 / 进任务栏 / 有标题栏），
  # 也就是这一段加进来之前所有插件的样子 —— **别给它们补一个空对象**，
  # 空对象与不写在这里是等价的，写了只会让人以为是必需的。
  if ($meta.window) { $manifest.window = $meta.window }
  # UTF-8 无 BOM：Rust 侧读清单时对 BOM 有容错，但**别依赖容错**
  $json = $manifest | ConvertTo-Json -Depth 6
  [IO.File]::WriteAllText((Join-Path $dir "lunac-plugin.json"), $json, (New-Object System.Text.UTF8Encoding($false)))

  # 随附文件（`notices`）：复制进**构建产物目录**。zip（第 ③ 步）与两个暂存目录（第 ④⑤ 步）
  # 都从 `$DistDir\<id>` 出货，所以在这里放一次就够（暂存那两步是整目录拷贝，天然带上）。
  # **缺文件 = 直接失败**：那是 Live2D 协议 5.1 的随附义务，宁可不打包也不能发一个
  # 没有授权文件的包 —— 漏掉这件事不会有任何别的症状，只会在法务上出问题。
  foreach ($d in $meta.dependencies) {
    foreach ($n in $d.notices) {
      $srcFile = Join-Path $Root $n.src
      if (-not (Test-Path $srcFile)) {
        throw "随附文件缺失：$($n.src) —— 见 pet 的 notices（协议 5.1 的随附义务，不能跳过）"
      }
      $target = Join-Path $dir $n.dest
      New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
      Copy-Item -Path $srcFile -Destination $target -Force
    }
  }
  Write-Host "  $id  v$pv" -ForegroundColor DarkGray
}

# ── ③ 打 zip ───────────────────────────────────────────────────────
# 用 ZipFile::CreateFromDirectory：包内是**目录内容**（lunac-plugin.json 在 zip 根），
# 这正是 install_from_bytes 认的形状之一。
Write-Host "[3/5] 打包 zip..." -ForegroundColor Yellow
Add-Type -AssemblyName System.IO.Compression.FileSystem
foreach ($id in $Ids) {
  $zip = Join-Path $OutDir "$id-$(Get-PluginVersion $id).zip"
  if (Test-Path $zip) { Remove-Item $zip -Force }
  [IO.Compression.ZipFile]::CreateFromDirectory(
    (Join-Path $DistDir $id), $zip,
    [IO.Compression.CompressionLevel]::Optimal, $false
  )
  $kb = [math]::Round((Get-Item $zip).Length / 1KB, 1)
  Write-Host "  $zip  ($kb KB)" -ForegroundColor Green
}

# ── ④⑤ 把「解开的插件目录」暂存到两个地方 ───────────────────────────
# 两处的形状完全相同（index.js + lunac-plugin.json + 依赖文件），所以收成一个函数：
#   · `release\ext-plugins\<id>\` —— `scripts\lunac-installer.nsi` 的「拓展插件」勾选段
#     直接从这儿拷进 `$INSTDIR\Modules\<id>\`（用户要求：拓展插件不默认装，但安装包里
#     要有勾选项）。用**解开**的目录而不是 zip：安装器没法解压；而且这份形状与
#     「市场装完」的盘上形状一致。
#   · `app\src-tauri\target\debug\Modules\<id>\` —— **dev 实例真正读的那一份**
#     （便携模式：根 = exe 所在目录，dev 的 exe 在 target\debug 下，见 verify-git.ps1 第 7 项）。
#     ⚠️ 少了这一步，`npm run tauri:dev` 跑的永远是**上一次**同步过去的插件。
#     2026-10-01 实测踩到：源码已经是 111 KB，dev 那份还停在 84 KB（差了快一天），
#     表现是「改了插件、dev 里怎么点都没反应」，而日志一行报错都没有 ——
#     **插件的改动必须先在 dev 里生效**，这是能精准调试的前提（用户 2026-10-01 要求）。
#     `release\ext-plugins\` 那份只服务安装包，它新鲜**不代表** dev 新鲜，两者不能互相顶替。
function Stage-PluginDir([string]$id, [string]$dst) {
  $meta = $Plugins[$id]
  $src = Join-Path $DistDir $id
  if (Test-Path $dst) { Remove-Item $dst -Recurse -Force }
  New-Item -ItemType Directory -Path $dst -Force | Out-Null
  Copy-Item -Path (Join-Path $src "*") -Destination $dst -Recurse -Force
  # `dependencies` 只对**市场安装**生效（宿主下载 + 校验 sha256）—— 这两条暂存路都**不会**下载。
  # 所以凡是有本机副本（`local`）的 `file` 依赖，必须在这里预先进目标目录；否则
  # 「安装器里勾了桌宠」的用户、以及 dev 实例，一开 Live2D 只会看到「引擎文件加载失败」。
  # **逐字节校验**：同一份文件好几条分发路，暂存的那份必须与清单钉死的 sha256 一致。
  foreach ($dep in $meta.dependencies) {
    if ($dep.type -ne "file" -or -not $dep.local) { continue }
    $file = Join-Path "$Root\release" $dep.local
    if (-not (Test-Path $file)) {
      Write-Host "  [!] 缺本机副本 $($dep.local) —— $id 的暂存里**没有**这份依赖（市场安装不受影响）" -ForegroundColor Yellow
      continue
    }
    $got = (Get-FileHash $file -Algorithm SHA256).Hash
    if ($got -ne $dep.sha256.ToUpper()) {
      throw "$($dep.local) 的 sha256 与清单钉死的不一致（期望 $($dep.sha256) / 实得 $got）—— 换版本时两处必须一起改"
    }
    $target = Join-Path $dst $dep.dest
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    Copy-Item -Path $file -Destination $target -Force
  }
}

$StageDir = "$Root\release\ext-plugins"
Write-Host "[4/5] 暂存给安装包（release\ext-plugins）..." -ForegroundColor Yellow
foreach ($id in $Ids) {
  Stage-PluginDir $id (Join-Path $StageDir $id)
  Write-Host "  release\ext-plugins\$id" -ForegroundColor DarkGray
}

# 只在 dev 产物已经存在时才同步（全新克隆、还没跑过 `npm run tauri:dev` / `cargo build` 时
# `target\debug` 可能根本不在）—— 不在就跳过并说明原因，这不是错误，只是还没到那一步。
$DevRoot = "$AppDir\src-tauri\target\debug"
if (Test-Path (Join-Path $DevRoot "lunac.exe")) {
  Write-Host "[5/5] 同步给 dev 实例（app\src-tauri\target\debug\Modules）..." -ForegroundColor Yellow
  foreach ($id in $Ids) {
    Stage-PluginDir $id (Join-Path $DevRoot "Modules\$id")
    Write-Host "  Modules\$id" -ForegroundColor DarkGray
  }
} else {
  Write-Host "[5/5] 跳过 dev 同步（还没有 target\debug\lunac.exe —— 先跑一次 npm run tauri:dev）" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "⓪ 桌宠的 Core 资产传过没有？那份 Release 资产**也是一次再分发** —— 要连着授权文件一起传：" -ForegroundColor Yellow
Write-Host "     gh release create live2dcubismcore-5.1.0 ^"
Write-Host "       release\deps\live2d\live2dcubismcore.min.js ^"
Write-Host "       licenses\live2d-cubism-core\LICENSE.md licenses\live2d-cubism-core\RedistributableFiles.txt ^"
Write-Host "       -R LythrumMoon/lunac-plugins -t 'Live2D Cubism Core 5.1.0'"
Write-Host "   · 三份缺一不可：只挂 Core 不挂授权文件 = 没满足协议 5.1 的随附条件" -ForegroundColor DarkGray
Write-Host ""
Write-Host "下一步：把它发布到**公开**插件仓库（终端用户要能免鉴权直链下载）。" -ForegroundColor Yellow
Write-Host "  powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1"
Write-Host "  · 默认目标仓库：LythrumMoon/lunac-plugins（公开；不存在时先用 gh repo create 建）"
Write-Host "  · 它会把 zip 放进 packages\ 并生成 index.json，然后 commit + push"
Write-Host "  · 宿主常量 INDEX_URL 指向该仓库的 index.json（见 src-tauri/src/plugin_market.rs）"
Write-Host ""
Write-Host "只想本机试装（不进市场）：把 zip 解到 <exe 根>\Modules\<id>\，再在 设置 → 插件 → 重新扫描。" -ForegroundColor DarkGray

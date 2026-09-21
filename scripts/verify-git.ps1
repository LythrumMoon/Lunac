# scripts/verify-git.ps1
# 「新克隆自检」—— 把本仓库克隆到新机器后先跑这个，一次列清缺什么。
#
# 用法:  powershell -ExecutionPolicy Bypass -File scripts\verify-git.ps1
#        （或仓库根执行  npm run verify）
#
# 退出码: 0 = 无阻断项（可启动）；1 = 存在 FAIL。
# 安全:   只判断 .env 里 AI_API_KEY 是否为空，绝不回显任何密钥内容。

$root = Split-Path -Parent $PSScriptRoot
$appDir = Join-Path $root "app"

$script:fail = 0
$script:warn = 0
function Ok($m)    { Write-Host "  [ OK ] $m" -ForegroundColor Green }
function Warn($m)  { Write-Host "  [WARN] $m" -ForegroundColor Yellow; $script:warn++ }
function Bad($m)   { Write-Host "  [FAIL] $m" -ForegroundColor Red;    $script:fail++ }
function Note($m)  { Write-Host "         $m" -ForegroundColor DarkGray }

Write-Host ""
Write-Host "Lunac 新克隆自检" -ForegroundColor Cyan
Write-Host "仓库根: $root"
Write-Host ""

# ── 1. 平台与工具链 ─────────────────────────────────────────────
Write-Host "[1/8] 平台与工具链" -ForegroundColor Cyan
if ($env:OS -eq "Windows_NT") {
  Ok "Windows（本项目仅支持 Windows：Win32 全局热键 / 系统托盘 / NSIS）"
} else {
  Warn "非 Windows 平台：编译与运行均不受支持"
}

foreach ($tool in @("node", "npm", "cargo")) {
  $cmd = Get-Command $tool -ErrorAction SilentlyContinue
  if ($cmd) {
    $ver = (& $tool --version 2>&1 | Select-Object -First 1)
    Ok "$tool → $ver"
  } else {
    Bad "$tool 未安装"
  }
}
if (Get-Command bun -ErrorAction SilentlyContinue) {
  Ok "bun → $(& bun --version 2>&1 | Select-Object -First 1)"
} else {
  Warn "未安装 bun（本仓库已改用 cargo 构建 agent 后端，通常不再需要）"
}
$nsis = Get-Command makensis -ErrorAction SilentlyContinue
if (-not $nsis) {
  $nsis = @(
    "${env:ProgramFiles}\NSIS\makensis.exe",
    "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
  ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if ($nsis) { Ok "makensis 已安装（可打 Setup.exe 安装包）" }
else { Warn "未安装 NSIS（仅打 Setup.exe 安装包时需要）" }

# ── 2. 前端依赖 ─────────────────────────────────────────────────
Write-Host "[2/8] 前端依赖" -ForegroundColor Cyan
if (Test-Path (Join-Path $appDir "node_modules")) {
  Ok "app\node_modules 已存在"
} else {
  Bad "app\node_modules 缺失"
  Note "修复: cd app ; npm install"
}

# ── 3. Agent 后端 ───────────────────────────────────────────────
Write-Host "[3/8] Agent 后端 core-agent\target\release\agent.exe" -ForegroundColor Cyan
$agent = Join-Path $root "core-agent\target\release\agent.exe"
if (Test-Path $agent) {
  Ok ("agent.exe 已编译（{0} MB）" -f [math]::Round((Get-Item $agent).Length / 1MB, 1))
} else {
  Warn "agent.exe 未编译 —— AI 对话将报 'agent.exe not found'"
  Note "修复: powershell -ExecutionPolicy Bypass -File scripts\build-core.ps1"
  Note "（等价于 cd core-agent ; cargo build --release；首次约需数分钟，产物约 2.5MB）"
}

# ── 4. AI 配置 ──────────────────────────────────────────────────
Write-Host "[4/8] AI 配置 app\src-tauri\.env" -ForegroundColor Cyan
$envFile = Join-Path $appDir "src-tauri\.env"
if (Test-Path $envFile) {
  $text = Get-Content $envFile -Raw
  if ([regex]::IsMatch($text, '(?m)^\s*AI_API_KEY\s*=\s*\S+')) {
    Ok "AI_API_KEY 已填写（内容不回显）"
  } else {
    Bad "AI_API_KEY 为空 → start_cli 会报 'No AI_API_KEY configured'"
  }
  if ([regex]::IsMatch($text, '(?m)^\s*AI_MODEL\s*=\s*\S+')) {
    Ok "AI_MODEL 已设置"
  } else {
    Warn "未设置 AI_MODEL（将回退默认 deepseek-flash）"
  }
} else {
  Bad "app\src-tauri\.env 缺失"
  Note "修复: Copy-Item app\src-tauri\.env.example app\src-tauri\.env  然后填 AI_API_KEY / AI_MODEL"
}

# ── 5. 离线 OCR 引擎（可选）────────────────────────────────────
Write-Host "[5/8] 离线 OCR 引擎 paddle-ocr\（可选）" -ForegroundColor Cyan
$paddleExe = Get-ChildItem -Path (Join-Path $root "paddle-ocr") -Recurse -Filter "PaddleOCR-json.exe" -ErrorAction SilentlyContinue |
  Select-Object -First 1
if ($paddleExe) {
  Ok "已找到 OCR 引擎: $($paddleExe.FullName)"
} else {
  Warn "未找到 OCR 引擎（不影响启动）"
  Note "首次使用 OCR 时界面会提示「下载并安装」；也可先跑 scripts\download-paddle-ocr.ps1"
}

# ── 6. 开发端口 ─────────────────────────────────────────────────
Write-Host "[6/8] 开发端口 5173（Vite strictPort）" -ForegroundColor Cyan
$busy = Get-NetTCPConnection -LocalPort 5173 -State Listen -ErrorAction SilentlyContinue
if ($busy) {
  $owner = ($busy | Select-Object -First 1).OwningProcess
  $pname = (Get-Process -Id $owner -ErrorAction SilentlyContinue).ProcessName
  Warn "5173 已被占用（PID $owner / $pname）—— strictPort 会直接启动失败，请先结束该进程"
} else {
  Ok "5173 空闲"
}

# ── 7. 数据目录 ─────────────────────────────────────────────────
Write-Host "[7/8] 数据目录（便携模式）" -ForegroundColor Cyan
Ok "根目录 = exe 所在目录；dev 落在 app\src-tauri\target\debug\ 旁，release 落在安装根"
Note "子目录: temp\（缓存）ModuleData\（历史/备忘录/自定义启动项）skills\ tools\ config\ paddle-ocr\"

# ── 8. 脚本编码（.ps1 / .nsi 的 UTF-8 BOM）──────────────────────
# 这条不是洁癖：含中文的 .ps1 / .nsi 一旦丢了 BOM，PS 5.1 与 makensis 会按 ANSI(GBK)
# 解码，中文字符会把紧随其后的引号/换行吞进双字节 —— 表现是「字符串缺少终止符」或
# makensis 的 `Bad text encoding`（行号还指向首个非 ASCII 行，极易误判）。
# 更隐蔽的是**两份 BOM**：PS 5.1 只吃掉第一份，剩下的 U+FEFF 让**首行变成一条命令**
# ⇒ 第 18 行的 param() 不再是首语句 ⇒ 脚本参数**全部不绑定**，而报错信息是
# 「无法将"?#"项识别为 cmdlet」+「无法将"param"项识别为 cmdlet」这种看不懂的东西
# （实测 build-release.ps1 就是这么坏掉的：版本号最后变成字符串 "False"）。
Write-Host "[8/8] 脚本编码（UTF-8 with BOM，且只有一份）" -ForegroundColor Cyan
$encFiles = Get-ChildItem -Path $root -Recurse -File -Include *.ps1, *.nsi -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -notmatch '\\(node_modules|target|release|dist|\.git|core|mingw64)\\' }
$encBad = 0
foreach ($f in $encFiles) {
  $b = [IO.File]::ReadAllBytes($f.FullName)
  $bom = $b.Length -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF
  $doubleBom = $bom -and $b.Length -ge 6 -and $b[3] -eq 0xEF -and $b[4] -eq 0xBB -and $b[5] -eq 0xBF
  $nonAscii = $false
  foreach ($x in $b) { if ($x -gt 0x7F) { $nonAscii = $true; break } }
  $rel = $f.FullName.Replace("$root\", "")
  if ($doubleBom) {
    Bad "$rel 开头有**两份** BOM"
    Note "修复: 以 UTF-8 with BOM 重存（首三字节 EF BB BF，第四字节不能又是 EF）"
    $encBad++
  } elseif ($nonAscii -and -not $bom) {
    Bad "$rel 含中文但没有 BOM"
    Note "修复: 以 UTF-8 with BOM 重存"
    $encBad++
  }
}
if ($encBad -eq 0) { Ok "$($encFiles.Count) 个 .ps1 / .nsi 编码正确" }

# ── 汇总 ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "════════════════════════════════════════" -ForegroundColor Cyan
if ($script:fail -eq 0) {
  Write-Host "自检通过（$script:warn 项提醒，不阻断）" -ForegroundColor Green
  Write-Host "启动开发模式:  cd app ; npm run tauri:dev      （或仓库根 npm run tauri:dev）" -ForegroundColor Cyan
} else {
  Write-Host "存在 $script:fail 项阻断 / $script:warn 项提醒 —— 先修 FAIL 再启动" -ForegroundColor Red
}
Write-Host ""

if ($script:fail -gt 0) { exit 1 } else { exit 0 }

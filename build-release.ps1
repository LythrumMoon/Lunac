﻿# Lunac Release Build Script
# Usage: .\build-release.ps1 [-Version <x.y.z>] [-NoBump]
#   .\build-release.ps1                   - 读 package.json 的版本，patch 自动 +1，并同步三处
#   .\build-release.ps1 -Version 0.10.0   - 显式指定版本（不递增），并同步三处
#   .\build-release.ps1 -NoBump           - 保持当前版本重新打包（调试用）
#
# Steps:
#   1. Pre-flight checks (cargo / makensis)
#   2. Kill existing processes
#   3. Build web assets (tsc + vite)
#   4. Build agent.exe (cargo build --release, core-agent/)
#   5. Build Rust binary (cargo build --release)
#   6. Copy binaries to release/Lunac/
#   7. Package VSCode extension (.vsix)
#   8. Stage PaddleOCR-json for offline OCR
#   9. Update NSI version + run makensis → Setup.exe

param(
  [string]$Version = "",
  [switch]$NoBump
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSCommandPath
$Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

# ── makensis 定位（NSIS 可能装在 x64 / x86 Program Files，或已在 PATH 中）──
$Makensis = @(
  "${env:ProgramFiles}\NSIS\makensis.exe",
  "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $Makensis) {
  $cmd = Get-Command makensis -ErrorAction SilentlyContinue
  if ($cmd) { $Makensis = $cmd.Source }
}

# ═══════════════════════════════════════════════════════════════════
# 1. Pre-flight checks
# ═══════════════════════════════════════════════════════════════════

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Lunac Release Build"                    -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ── Version：自动递增 + 三处同步（2026-09-17 改）────────────────────
# 版本号在三处各写了一份，只改一处就会产出「安装包叫 0.9.1、exe 属性里还是 0.9.0」
# 这种自相矛盾的包。所以这里统一：**递增一次 → 三处全部写回**。
#
# **必须在第 5 步 `cargo build` 之前**做完：`Cargo.toml` 的 version 参与编译，
# 改晚了 exe 里的版本就与安装包名不一致（tauri.conf.json 的 version 也会被嵌进 exe）。
$VersionFiles = @{
  PackageJson = "$Root\app\package.json"
  TauriConf   = "$Root\app\src-tauri\tauri.conf.json"
  CargoToml   = "$Root\app\src-tauri\Cargo.toml"
}

function Get-DeclaredVersion {
  # package.json 是唯一真相源（历史原因：本脚本一直读它）
  if (-not (Test-Path $VersionFiles.PackageJson)) {
    throw "package.json not found at $($VersionFiles.PackageJson)"
  }
  $v = (Get-Content $VersionFiles.PackageJson -Raw | ConvertFrom-Json).version
  if (-not $v) { throw "Could not read version from package.json" }
  return $v
}

function Set-VersionEverywhere([string]$ver) {
  if ($ver -notmatch '^\d+\.\d+\.\d+$') {
    throw "版本号必须形如 x.y.z，收到：$ver"
  }
  # 一律用「正则替换**第一处**匹配」，不用「解析 JSON 再整体写回」：后者会重排字段、
  # 改缩进，diff 里全是无关改动。
  # 编码统一 **UTF-8 无 BOM**：JSON / TOML 带 BOM 会让 cargo 报 unexpected character。
  $utf8 = New-Object System.Text.UTF8Encoding($false)
  $edits = @(
    @{ Path = $VersionFiles.PackageJson; Pattern = '(?m)^(\s*"version"\s*:\s*")[^"]+(")' },
    @{ Path = $VersionFiles.TauriConf;   Pattern = '(?m)^(\s*"version"\s*:\s*")[^"]+(")' },
    @{ Path = $VersionFiles.CargoToml;   Pattern = '(?m)^(version\s*=\s*")[^"]+(")' }
  )
  foreach ($e in $edits) {
    $text = [IO.File]::ReadAllText($e.Path)
    $re = [regex]::new($e.Pattern)
    if (-not $re.IsMatch($text)) {
      throw "在 $($e.Path) 里找不到可替换的版本号字段"
    }
    [IO.File]::WriteAllText($e.Path, $re.Replace($text, "`${1}$ver`${2}", 1), $utf8)
    Write-Host "    $($e.Path.Replace("$Root\", ''))  ->  $ver" -ForegroundColor DarkGray
  }
}

$CurrentVersion = Get-DeclaredVersion
if ($Version) {
  # 显式指定：只同步、不递增（重复打同一个版本时用）
  if ($Version -ne $CurrentVersion) {
    Write-Host "  版本同步到指定值：$CurrentVersion -> $Version" -ForegroundColor Green
    Set-VersionEverywhere $Version
  }
} elseif ($NoBump) {
  $Version = $CurrentVersion
  Write-Host "  -NoBump：保持 $Version 重新打包" -ForegroundColor DarkGray
} else {
  if ($CurrentVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw "当前版本号不是 x.y.z，无法自动递增：$CurrentVersion（请显式传 -Version 或先修正）"
  }
  $parts = $CurrentVersion.Split('.')
  $Version = "$($parts[0]).$($parts[1]).$([int]$parts[2] + 1)"
  Write-Host "  版本自动递增：$CurrentVersion -> $Version" -ForegroundColor Green
  Set-VersionEverywhere $Version
}
Write-Host "  Version : $Version" -ForegroundColor White
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ── Tool checks ────────────────────────────────────────────────────
Write-Host "[1/9] Pre-flight checks..." -ForegroundColor Yellow

$Checks = @{
  "cargo"    = { cargo --version 2>&1 | Out-Null; $LASTEXITCODE -eq 0 }
  "makensis" = { [bool]$Makensis }
}

$AllOk = $true
foreach ($tool in $Checks.Keys) {
  $ok = & $Checks[$tool]
  if ($ok) {
    Write-Host "  [OK]  $tool" -ForegroundColor Green
  } else {
    Write-Host "  [MISS] $tool — install before building" -ForegroundColor Red
    $AllOk = $false
  }
}

# ── Source dirs ────────────────────────────────────────────────────
$RequiredDirs = @(
  "$Root\app",
  "$Root\app\src-tauri",
  "$Root\core-agent",
  "$Root\release"
)
foreach ($dir in $RequiredDirs) {
  if (-not (Test-Path $dir)) {
    Write-Host "  [MISS] $dir" -ForegroundColor Red
    $AllOk = $false
  }
}

if (-not $AllOk) {
  throw "Pre-flight checks failed. Fix missing dependencies before building."
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 2. Kill existing processes (they may hold file locks)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[2/9] Killing running processes..." -ForegroundColor Yellow
$Killed = $false
foreach ($name in @("lunac", "agent")) {
  $proc = Get-Process -Name $name -ErrorAction SilentlyContinue
  if ($proc) {
    taskkill /F /IM "$name.exe" 2>$null | Out-Null
    Write-Host "  Killed $name.exe" -ForegroundColor DarkGray
    $Killed = $true
  }
}
if (-not $Killed) {
  Write-Host "  (no running processes)" -ForegroundColor DarkGray
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 3. Build web assets (TypeScript + Vite)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[3/9] Building web assets (tsc + vite)..." -ForegroundColor Yellow
Push-Location "$Root\app"
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  npm run build
  if ($LASTEXITCODE -ne 0) { throw "Web build failed (exit $LASTEXITCODE)" }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 4. Build agent.exe (self-developed agent backend, core-agent/)
#    Must run BEFORE the Rust build: tauri.conf.json 的 bundle.resources
#    指向 core-agent\target\release\agent.exe，缺文件会让打包失败。
# ═══════════════════════════════════════════════════════════════════

Write-Host "[4/9] Building agent.exe (cargo build --release, core-agent)..." -ForegroundColor Yellow
Push-Location "$Root\core-agent"
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  cargo build --release
  if ($LASTEXITCODE -ne 0) { throw "agent.exe build failed (exit $LASTEXITCODE)" }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 5. Build Rust release binary (lunac.exe)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[5/9] Building Rust binary (cargo build --release)..." -ForegroundColor Yellow
Push-Location "$Root\app\src-tauri"
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  cargo build --release
  if ($LASTEXITCODE -ne 0) { throw "Rust build failed (exit $LASTEXITCODE)" }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 6. Verify & copy binaries to release/Lunac/
# ═══════════════════════════════════════════════════════════════════

Write-Host "[6/9] Copying binaries to release/Lunac/..." -ForegroundColor Yellow

$ReleaseDir = "$Root\release"
$AppDir     = "$ReleaseDir\Lunac"

# 干净暂存：每次构建都从空目录开始。
# 否则上一版的遗留文件会被静默打进新包 —— 历史上 `cli.exe`（上游 Claude Code CLI
# 的 bun 编译产物，.gitignore 明确禁止再分发）就是这么一路跟进每个安装包的，
# 而 NSI 只打 agent.exe 之后它就成了「打不上也删不掉」的僵尸文件。
if (Test-Path $AppDir) {
  Remove-Item -Recurse -Force $AppDir
  Write-Host "  Cleared stale staging dir (fresh build)" -ForegroundColor DarkGray
}
New-Item -ItemType Directory $AppDir -Force | Out-Null

$Binaries = @{
  "lunac.exe"           = "$Root\app\src-tauri\target\release\lunac.exe"
  "agent.exe"           = "$Root\core-agent\target\release\agent.exe"
  "WebView2Loader.dll"  = "$Root\app\src-tauri\target\release\WebView2Loader.dll"
}

$TotalBinSize = 0
foreach ($name in $Binaries.Keys) {
  $src = $Binaries[$name]
  if (-not (Test-Path $src)) {
    throw "Binary not found: $src"
  }
  Copy-Item $src "$AppDir\$name" -Force
  $size = (Get-Item $src).Length
  $TotalBinSize += $size
  $sizeMb = [math]::Round($size / 1MB, 1)
  Write-Host "  $name  $sizeMb MB" -ForegroundColor DarkGray
}

# 打包前防空转：暂存目录里绝不能有上游 cli.exe。
# （NSI 的 File 指令会在编译期拦住「文件不存在」，但拦不住「多打了不该打的文件」。）
if (Test-Path "$AppDir\cli.exe") {
  throw "暂存目录里出现 cli.exe —— 上游 CLI 禁止随安装包分发（见 .gitignore / ai-spec §8.3）"
}

# ── 技能 / 工具模板（agent-templates\ → skills\ + tools\）─────────────
# 装完就有的两个目录，用户照着 README 与 .example 抄自己的技能/工具。
# 刻意只放「不可加载」的形态：
#   · skills\ 下任何含 SKILL.md 的子目录都会被列进系统提示词
#   · tools\  下任何 .json 都会被当工具加载
# 所以模板一律用 .example 后缀，避免污染模型的工具清单与提示词。
$TplDir = "$Root\agent-templates"
foreach ($sub in @("skills", "tools")) {
  $src = Join-Path $TplDir $sub
  if (-not (Test-Path $src)) {
    throw "agent-templates\$sub not found: $src"
  }
  Copy-Item $src -Destination "$AppDir\$sub" -Recurse -Force
  Write-Host "  $sub\ (README + .example templates)" -ForegroundColor DarkGray
}

$totalMb = [math]::Round($TotalBinSize / 1MB, 1)
Write-Host "  Total : $totalMb MB" -ForegroundColor Green
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 7. Package VSCode extension (.vsix) + copy to release/Lunac/
# ═══════════════════════════════════════════════════════════════════
# The Lunac desktop app's "Attach to VSCode" button opens VSCode and
# auto-installs the extension if the .vsix is found next to lunac.exe.
# Users can also install it manually from the install directory.

$VsixSrc = "$Root\vscode-extension"
$VsixOut = "$AppDir\lunac.vsix"

Write-Host "[7/9] VSCode extension (.vsix)..." -ForegroundColor Yellow

if (Test-Path $VsixSrc) {
  try {
    Push-Location $VsixSrc

    # 1) Prefer a pre-built .vsix whose filename embeds the target version.
    #    (Get-ChildItem default order is alphabetical — a stale older .vsix
    #    (e.g. lunac-0.6.0.vsix) sorts first and used to get copied even when
    #    it did not match the app version. Match by version instead.)
    $vsixFiles = Get-ChildItem -Path . -Filter "*.vsix" -File
    $vsix = $vsixFiles |
      Where-Object { $_.Name -match [regex]::Escape($Version) } |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1

    # 2) No version-matched package exists → build one now so the shipped
    #    extension always matches the app version.
    if (-not $vsix) {
      # Install dependencies first (first build or node_modules missing)
      if (-not (Test-Path "node_modules\@vscode\vsce")) {
        Write-Host "  Installing vsce..." -ForegroundColor DarkGray
        npm install --no-audit --no-fund 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
          Write-Host "  npm install failed (exit $LASTEXITCODE)" -ForegroundColor Yellow
        }
      }
      if (Test-Path "node_modules\@vscode\vsce") {
        Write-Host "  Building with vsce package (v$Version)..." -ForegroundColor DarkGray
        # Use & to invoke local node_modules/.bin/vsce directly (avoids npx cache)
        $vsceCmd = Join-Path $VsixSrc "node_modules\.bin\vsce.cmd"
        & $vsceCmd package --allow-missing-repository 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
        if ($LASTEXITCODE -eq 0) {
          $vsix = Get-ChildItem -Path . -Filter "*.vsix" -File |
            Where-Object { $_.Name -match [regex]::Escape($Version) } |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1
        } else {
          Write-Host "  vsce package failed (exit $LASTEXITCODE) — manual: cd vscode-extension && npm run package" -ForegroundColor Yellow
        }
      }
    }

    # 3) Copy the chosen package, or fall back to newest with a WARN.
    if ($vsix) {
      Copy-Item $vsix.FullName $VsixOut -Force
      $vsixSize = [math]::Round($vsix.Length / 1KB, 1)
      Write-Host "  Copied $($vsix.Name) → lunac.vsix ($vsixSize KB)" -ForegroundColor Green
    } elseif ($vsixFiles) {
      $fallback = $vsixFiles | Sort-Object LastWriteTime -Descending | Select-Object -First 1
      Write-Host "  WARN: no v$Version .vsix available; copying newest $($fallback.Name)" -ForegroundColor Yellow
      Copy-Item $fallback.FullName $VsixOut -Force
    } else {
      Write-Host "  No .vsix found (extension not packaged)" -ForegroundColor DarkGray
    }
  } finally {
    Pop-Location
  }
} else {
  Write-Host "  vscode-extension/ not found, skipping" -ForegroundColor DarkGray
}

# If we have the .vsix at the destination, verify it
if (Test-Path $VsixOut) {
  Write-Host "  Verified: $VsixOut" -ForegroundColor Green
} else {
  Write-Host "  Skipped (vscode-extension not built — 'Attach to VSCode' will open VSCode without extension auto-install)" -ForegroundColor DarkGray
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 8. Stage PaddleOCR-json (offline OCR engine)
# ═══════════════════════════════════════════════════════════════════
# Priority: 1) copy from local paddle-ocr/  2) download from GitHub
# Always does a fresh copy — no stale/partial copies from previous builds.

Write-Host "[8/9] PaddleOCR-json (offline OCR)..." -ForegroundColor Yellow
$PaddleDir = "$AppDir\paddle-ocr"

# 定位 7z.exe：优先 Program Files 两处，其次 PATH。找不到返回 $null
# （步骤 8 解 .7z 与步骤 9 校验包内容都用它）。
function Find-SevenZip {
  $candidates = @(
    "$env:ProgramFiles\7-Zip\7z.exe",
    "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
  )
  $exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $exe) {
    $cmd = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if ($cmd) { $exe = $cmd.Source }
  }
  return $exe
}

# 解压 .7z：优先 7z.exe，其次系统自带 bsdtar（Windows 10 1803+）。
# PowerShell 的 Expand-Archive 不支持 7z，而 PaddleOCR-json 的 Windows 资产是 .7z。
function Expand-SevenZip {
  param([string]$Archive, [string]$Destination)
  $exe = Find-SevenZip
  if ($exe) {
    & $exe x $Archive "-o$Destination" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7z exited with code $LASTEXITCODE" }
    return
  }
  & tar -xf $Archive -C $Destination
  if ($LASTEXITCODE -ne 0) {
    throw "解压 $Archive 失败：请安装 7-Zip 后重试（tar 返回 $LASTEXITCODE）"
  }
}

# Always start clean — remove any stale/partial copy from previous builds.
# The "Already staged" shortcut was unreliable: if a previous build was
# interrupted after copying the exe but before models/DLLs, the partial
# copy would be skipped on next run, producing a broken release folder.
if (Test-Path $PaddleDir) {
  Remove-Item -Recurse -Force $PaddleDir
  Write-Host "  Removed previous paddle-ocr/ (fresh build)" -ForegroundColor DarkGray
}

# Priority 1: copy from local source (dev repo already has PaddleOCR-json)
$LocalPaddle = "$Root\paddle-ocr\PaddleOCR-json\PaddleOCR-json_v1.4.1"
if (Test-Path (Join-Path $LocalPaddle "PaddleOCR-json.exe")) {
  Write-Host "  Copying from local paddle-ocr/..."
  # Copy directory CONTENTS (not the wrapper folder itself) to produce flat structure:
  #   release/Lunac/paddle-ocr/PaddleOCR-json.exe  (not .../PaddleOCR-json_v1.4.1/...)
  New-Item -ItemType Directory -Path $PaddleDir -Force | Out-Null
  Copy-Item "$LocalPaddle\*" -Destination $PaddleDir -Recurse -Force

  # Verify key files were copied (exe + models config + core runtime DLLs)
  $paddleSize = [math]::Round((Get-ChildItem $PaddleDir -Recurse | Measure-Object Length -Sum).Sum / 1MB, 1)
  $exeExists = Test-Path (Join-Path $PaddleDir "PaddleOCR-json.exe")
  $modelsExist = Test-Path (Join-Path $PaddleDir "models\config_chinese.txt")
  $dllExists = Test-Path (Join-Path $PaddleDir "paddle_inference.dll")
  if ($exeExists -and $modelsExist -and $dllExists) {
    Write-Host "  Copied ($paddleSize MB) — verified OK" -ForegroundColor Green
  } else {
    throw "PaddleOCR-json copy incomplete: exe=$exeExists models=$modelsExist paddle_inference.dll=$dllExists"
  }
} else {
  # Priority 2: download from GitHub releases
  # NOTE: the v1.4.1 Windows asset is a .7z (no .zip); Expand-Archive cannot read it.
  $PaddleVersion = "v1.4.1"
  $PaddleArchive = "PaddleOCR-json_v1.4.1_windows_x64.7z"
  $PaddleUrl = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/$PaddleVersion/$PaddleArchive"
  $TempArchive = "$env:TEMP\$PaddleArchive"
  $TempExtract = "$env:TEMP\paddle-ocr-extract"

  try {
    Write-Host "  Downloading $PaddleVersion from GitHub..."
    Invoke-WebRequest -Uri $PaddleUrl -OutFile $TempArchive -UseBasicParsing
    Write-Host "  Download complete." -ForegroundColor DarkGray

    if (Test-Path $TempExtract) { Remove-Item -Recurse -Force $TempExtract }
    New-Item -ItemType Directory -Path $TempExtract -Force | Out-Null
    Expand-SevenZip -Archive $TempArchive -Destination $TempExtract

    # The archive contains a PaddleOCR-json_v1.4.1/ folder; copy its contents flat
    New-Item -ItemType Directory -Path $PaddleDir -Force | Out-Null
    $innerDir = Get-ChildItem -Path $TempExtract -Directory | Select-Object -First 1
    if ($innerDir) {
      Copy-Item "$($innerDir.FullName)\*" -Destination $PaddleDir -Recurse -Force
    } else {
      Copy-Item "$TempExtract\*" -Destination $PaddleDir -Recurse -Force
    }

    # Verify
    $paddleSize = [math]::Round((Get-ChildItem $PaddleDir -Recurse | Measure-Object Length -Sum).Sum / 1MB, 1)
    if (-not (Test-Path (Join-Path $PaddleDir "PaddleOCR-json.exe"))) {
      throw "PaddleOCR-json.exe not found after extract"
    }
    Write-Host "  Installed ($paddleSize MB) — verified OK" -ForegroundColor Green
  } catch {
    Write-Warning "  PaddleOCR-json download failed: $_"
    Write-Warning "  OCR will not be available in this build."
    if (Test-Path $PaddleDir) { Remove-Item -Recurse -Force $PaddleDir -ErrorAction SilentlyContinue }
  } finally {
    Remove-Item -Recurse -Force $TempExtract -ErrorAction SilentlyContinue
    Remove-Item $TempArchive -ErrorAction SilentlyContinue
  }
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 9. Update NSI version + run makensis → Setup.exe
# ═══════════════════════════════════════════════════════════════════

Write-Host "[9/9] Running makensis..." -ForegroundColor Yellow

# NSI 位于 scripts\（已入库；release\ 整体被 gitignore，放那儿换个克隆就跑不了）。
# NSI 内部用 `!cd ${__FILEDIR__}\..\release` 自己锚定了源文件目录 —— makensis 解析
# File / OutFile 的相对路径时用的是**脚本所在目录**，不是调用方 CWD，所以路径解析
# 与这里的 Push-Location 无关（保留它只是双保险）。实测：从仓库根调用也能正确解析。
$NsiFile = "$Root\scripts\lunac-installer.nsi"
if (-not (Test-Path $NsiFile)) {
  throw "NSI script not found: $NsiFile"
}

# Read + replace version using UTF8 (no BOM) — NSIS compiler requires correct encoding
$NsiContent = [System.IO.File]::ReadAllText($NsiFile, [System.Text.Encoding]::UTF8)

# Replace PRODUCT_VERSION define (OutFile/DisplayVersion use ${PRODUCT_VERSION})
$NewDefine = "!define PRODUCT_VERSION `"$Version`""
$NsiContent = $NsiContent -replace '!define PRODUCT_VERSION "[\d.]+"', $NewDefine

# Verify replacement happened
if ($NsiContent -notmatch [regex]::Escape($NewDefine)) {
  throw "Failed to update PRODUCT_VERSION in NSI script"
}

[System.IO.File]::WriteAllText($NsiFile, $NsiContent, [System.Text.UTF8Encoding]::new($true))
Write-Host "  Version in NSI: $Version" -ForegroundColor DarkGray

if (-not (Test-Path $Makensis)) {
  throw "makensis not found（NSIS 未安装或不在默认路径；已尝试 Program Files / x86 与 PATH）"
}

# 覆盖同名旧产物前先删干净：makensis 打不开已存在的输出文件时只会含糊地报
# "Can't open output file"（实测：上一版包刚生成、杀软还在实时扫描它那会儿）。
# 删不掉就说明确实被占用，这里直接给出可读的原因，省得对着 makensis 的报错猜。
$SetupPath = "$ReleaseDir\Lunac-$Version-Setup.exe"
if (Test-Path $SetupPath) {
  try {
    Remove-Item $SetupPath -Force -ErrorAction Stop
    Write-Host "  Removed previous $Version installer" -ForegroundColor DarkGray
  } catch {
    throw "无法删除上一版安装包（疑似被杀软或其他进程占用），请关闭后重试：$SetupPath`n$($_.Exception.Message)"
  }
}

Push-Location $ReleaseDir
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $output = & $Makensis $NsiFile 2>&1
  Write-Host $output -ForegroundColor DarkGray
  if ($LASTEXITCODE -ne 0) {
    throw "makensis failed (exit $LASTEXITCODE)"
  }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }

# Remove stale literal-name file from previous broken builds
$StalePath = "$ReleaseDir\Lunac-`${PRODUCT_VERSION}-Setup.exe"
if (Test-Path $StalePath) {
  Remove-Item $StalePath -Force
  Write-Host "  Removed stale: $StalePath" -ForegroundColor DarkGray
}

if (-not (Test-Path $SetupPath)) {
  # Try glob (exclude broken literal-variable filenames)
  $matches = Get-ChildItem $ReleaseDir "Lunac-*-Setup.exe" |
    Where-Object { $_.Name -notlike '*`$*' -and $_.Name -notlike '*${*' } |
    Sort-Object LastWriteTime -Descending
  if ($matches) {
    $SetupPath = $matches[0].FullName
    Write-Host "  Found installer (alt name): $SetupPath" -ForegroundColor Yellow
  } else {
    throw "Setup.exe not produced in $ReleaseDir — check makensis output above for errors"
  }
}

# ── 包内容校验：必须含 agent.exe、不得含 cli.exe ─────────────────────
# NSIS 的文件表是 LZMA 压缩的，直接扫 Setup.exe 字节不可靠，所以用 7z 列包内清单。
# 没装 7z 就跳过并提示（编译期的 File 指令已经能拦住「文件不存在」，
# 这里防的是「打了不该打的东西」）。
$SevenZip = Find-SevenZip
if ($SevenZip) {
  # 7z 在非 TTY 下可能按宽度折断长文件名，先压掉所有空白再匹配
  $packed = ((& $SevenZip l $SetupPath 2>&1) -join "`n") -replace '\s+', ''
  if ($packed -match 'cli\.exe') {
    throw "安装包里出现了 cli.exe（上游 CLI，禁止再分发）：$SetupPath"
  }
  if ($packed -notmatch 'agent\.exe') {
    throw "安装包里没有 agent.exe（自研 AI 后端）—— 装完 AI 起不来：$SetupPath"
  }
  Write-Host "  包内容 OK：含 agent.exe，无 cli.exe" -ForegroundColor Green
} else {
  Write-Host "  [SKIP] 未找到 7z.exe，跳过包内容校验" -ForegroundColor Yellow
}

# ═══════════════════════════════════════════════════════════════════
# Done
# ═══════════════════════════════════════════════════════════════════

$Stopwatch.Stop()
$TotalSec = [math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)
$SetupSize = [math]::Round((Get-Item $SetupPath).Length / 1MB, 1)

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Build complete — $TotalSec s"         -ForegroundColor Cyan
Write-Host "  $SetupPath"                                -ForegroundColor White
Write-Host "  Size : $SetupSize MB"                      -ForegroundColor White
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

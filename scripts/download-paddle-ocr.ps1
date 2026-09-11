# Download PaddleOCR-json v1.4.1 for release packaging
# Called by tauri-build.ps1 → extracts to release/Lunac/paddle-ocr/
#
# NOTE: the v1.4.1 Windows asset is a .7z (there is NO .zip for Windows x64).
# PowerShell's Expand-Archive cannot read 7z, so use 7-Zip when available and
# otherwise fall back to Windows' bundled bsdtar (libarchive reads 7z).

param(
  [string]$ReleaseDir = $null
)

$version = "v1.4.1"
$archiveName = "PaddleOCR-json_v1.4.1_windows_x64.7z"
$url = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/$version/$archiveName"

# 解压 .7z：优先 7z.exe，其次系统自带 bsdtar（Windows 10 1803+）
function Expand-SevenZip {
  param([string]$Archive, [string]$Destination)
  $candidates = @(
    "$env:ProgramFiles\7-Zip\7z.exe",
    "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
  )
  $exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $exe) {
    $cmd = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if ($cmd) { $exe = $cmd.Source }
  }
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

if (-not $ReleaseDir) {
  $ReleaseDir = Join-Path $PSScriptRoot "..\release\Lunac"
}
$ReleaseDir = (Resolve-Path $ReleaseDir -ErrorAction SilentlyContinue).Path
if (-not $ReleaseDir) {
  New-Item -ItemType Directory -Path "$PSScriptRoot\..\release\Lunac" -Force | Out-Null
  $ReleaseDir = (Resolve-Path "$PSScriptRoot\..\release\Lunac").Path
}

$paddleDir = Join-Path $ReleaseDir "paddle-ocr"

# Skip if already present
if (Test-Path (Join-Path $paddleDir "PaddleOCR-json.exe")) {
  Write-Host "[paddle-ocr] Already present in $paddleDir, skipping."
  exit 0
}

Write-Host "[paddle-ocr] Downloading $version from GitHub..."
$tempArchive = Join-Path $env:TEMP "PaddleOCR-json.7z"
$tempExtract = Join-Path $env:TEMP "paddle-ocr-extract"

try {
  # Download with progress
  Invoke-WebRequest -Uri $url -OutFile $tempArchive -UseBasicParsing

  # Extract
  if (Test-Path $tempExtract) { Remove-Item -Recurse -Force $tempExtract }
  New-Item -ItemType Directory -Path $tempExtract -Force | Out-Null
  Expand-SevenZip -Archive $tempArchive -Destination $tempExtract

  # The archive wraps everything in a PaddleOCR-json_v1.4.1/ folder — move it out
  $innerDir = Get-ChildItem -Path $tempExtract -Directory | Select-Object -First 1
  if ($innerDir) {
    if (Test-Path $paddleDir) { Remove-Item -Recurse -Force $paddleDir }
    Move-Item -Path $innerDir.FullName -Destination $paddleDir -Force
  } else {
    # Files at root (unlikely, but handle it)
    New-Item -ItemType Directory -Path $paddleDir -Force | Out-Null
    Get-ChildItem -Path $tempExtract | Move-Item -Destination $paddleDir -Force
  }

  Write-Host "[paddle-ocr] Installed to $paddleDir" -ForegroundColor Green
} catch {
  Write-Error "[paddle-ocr] Download failed: $_"
  exit 1
} finally {
  Remove-Item -Recurse -Force $tempExtract -ErrorAction SilentlyContinue
  Remove-Item $tempArchive -ErrorAction SilentlyContinue
}

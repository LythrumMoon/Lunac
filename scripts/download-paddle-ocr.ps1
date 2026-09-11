# Download PaddleOCR-json v1.4.1 for release packaging
# Called by tauri-build.ps1 → extracts to release/Lunac/paddle-ocr/

param(
  [string]$ReleaseDir = $null
)

$version = "v1.4.1"
$archiveName = "PaddleOCR-json_v1.4.1_windows_x64.zip"
$url = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/$version/$archiveName"

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
$tempZip = Join-Path $env:TEMP "PaddleOCR-json.zip"
$tempExtract = Join-Path $env:TEMP "paddle-ocr-extract"

try {
  # Download with progress
  Invoke-WebRequest -Uri $url -OutFile $tempZip -UseBasicParsing

  # Extract
  if (Test-Path $tempExtract) { Remove-Item -Recurse -Force $tempExtract }
  Expand-Archive -Path $tempZip -DestinationPath $tempExtract -Force

  # GitHub archives wrap in an inner directory — move its contents
  $innerDir = Get-ChildItem -Path $tempExtract -Directory | Select-Object -First 1
  if ($innerDir) {
    if (Test-Path $paddleDir) { Remove-Item -Recurse -Force $paddleDir }
    Move-Item -Path $innerDir.FullName -Destination $paddleDir -Force
  } else {
    # Files at root (unlikely for GitHub releases, but handle it)
    New-Item -ItemType Directory -Path $paddleDir -Force | Out-Null
    Get-ChildItem -Path $tempExtract | Move-Item -Destination $paddleDir -Force
  }

  Write-Host "[paddle-ocr] Installed to $paddleDir" -ForegroundColor Green
} catch {
  Write-Error "[paddle-ocr] Download failed: $_"
  exit 1
} finally {
  Remove-Item -Recurse -Force $tempExtract -ErrorAction SilentlyContinue
  Remove-Item $tempZip -ErrorAction SilentlyContinue
}

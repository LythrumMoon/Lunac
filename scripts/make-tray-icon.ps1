# scripts/make-tray-icon.ps1 -- build Lunac icon assets from one source image.
# ASCII only: PowerShell 5.1 decodes BOM-less UTF-8 as GBK.
#
# Outputs:
#   app-flutter/assets/tray_icon.png                 64x64 PNG for tray_manager
#   app-flutter/windows/runner/resources/app_icon.ico  multi-size ICO for exe / window / taskbar
#
# The source photo is cropped to a centered square (biased toward the top by -TopBias
# for portrait images) and then resized with high-quality bicubic sampling.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\make-tray-icon.ps1 -Source <path-to-image>

param(
    [Parameter(Mandatory = $true)][string]$Source,
    [string]$OutDir,
    [string]$IcoOut,
    [double]$TopBias = 0.30,
    [int]$Size = 64,
    [int[]]$IcoSizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

if (-not (Test-Path -LiteralPath $Source)) {
    throw "source image not found: $Source"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) {
    $OutDir = Join-Path $repoRoot "app-flutter\assets"
}
if (-not $IcoOut) {
    $IcoOut = Join-Path $repoRoot "app-flutter\windows\runner\resources\app_icon.ico"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $IcoOut) | Out-Null

function New-SquareBitmap(
    [System.Drawing.Image]$Image,
    [System.Drawing.Rectangle]$SrcRect,
    [int]$Px
) {
    $bmp = New-Object System.Drawing.Bitmap -ArgumentList $Px, $Px
    $graphics = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $dst = New-Object System.Drawing.Rectangle -ArgumentList 0, 0, $Px, $Px
        $graphics.DrawImage($Image, $dst, $SrcRect, [System.Drawing.GraphicsUnit]::Pixel)
    } finally {
        $graphics.Dispose()
    }
    return $bmp
}

$img = [System.Drawing.Image]::FromFile((Resolve-Path -LiteralPath $Source).Path)
try {
    $side = [Math]::Min($img.Width, $img.Height)
    $x = [int](($img.Width - $side) / 2)
    $y = [int](($img.Height - $side) * $TopBias)
    $srcRect = New-Object System.Drawing.Rectangle -ArgumentList $x, $y, $side, $side

    # 1) Tray icon: one square PNG loaded by tray_manager.
    $tray = New-SquareBitmap -Image $img -SrcRect $srcRect -Px $Size
    $trayPath = Join-Path $OutDir "tray_icon.png"
    $tray.Save($trayPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $tray.Dispose()
    Write-Host "tray icon written: $trayPath (${Size}x${Size})"

    # 2) App icon: multi-size ICO (PNG frames, supported by Windows Vista and later).
    #    The ICO feeds IDI_APP_ICON in Runner.rc, so exe / window / taskbar share one icon.
    $frames = New-Object System.Collections.ArrayList
    foreach ($px in ($IcoSizes | Sort-Object)) {
        $bmp = New-SquareBitmap -Image $img -SrcRect $srcRect -Px $px
        $stream = New-Object System.IO.MemoryStream
        $bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        [void]$frames.Add([pscustomobject]@{ Size = $px; Bytes = $stream.ToArray() })
        $stream.Dispose()
    }

    # Write the ICO byte by byte: BinaryWriter overload resolution in PowerShell can pick
    # the 4-byte Write(int) for a UInt16 and shift the whole header (verified failure).
    $bytes = New-Object System.Collections.Generic.List[byte]
    $bytes.Add(0)                              # reserved
    $bytes.Add(0)
    $bytes.Add(1)                              # type: icon
    $bytes.Add(0)
    $count = $frames.Count
    $bytes.Add([byte]($count -band 0xFF))      # image count (little endian)
    $bytes.Add([byte](($count -shr 8) -band 0xFF))

    $offset = 6 + 16 * $count
    foreach ($frame in $frames) {
        $dim = if ($frame.Size -ge 256) { 0 } else { $frame.Size }
        $bytes.Add([byte]$dim)                 # width (0 = 256)
        $bytes.Add([byte]$dim)                 # height (0 = 256)
        $bytes.Add(0)                          # palette size
        $bytes.Add(0)                          # reserved
        $bytes.Add(1)                          # color planes = 1
        $bytes.Add(0)
        $bytes.Add(32)                         # bits per pixel = 32
        $bytes.Add(0)
        $length = $frame.Bytes.Length
        $bytes.Add([byte]($length -band 0xFF))
        $bytes.Add([byte](($length -shr 8) -band 0xFF))
        $bytes.Add([byte](($length -shr 16) -band 0xFF))
        $bytes.Add([byte](($length -shr 24) -band 0xFF))
        $bytes.Add([byte]($offset -band 0xFF))
        $bytes.Add([byte](($offset -shr 8) -band 0xFF))
        $bytes.Add([byte](($offset -shr 16) -band 0xFF))
        $bytes.Add([byte](($offset -shr 24) -band 0xFF))
        $offset += $length
    }
    foreach ($frame in $frames) {
        $bytes.AddRange([byte[]]$frame.Bytes)
    }
    [System.IO.File]::WriteAllBytes($IcoOut, $bytes.ToArray())
    Write-Host "app icon written: $IcoOut ($count sizes)"
} finally {
    $img.Dispose()
}

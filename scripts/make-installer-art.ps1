# Generate Lunac installer bitmaps (24-bit BMP, NSIS MUI sizes).
#
#   scripts\installer-welcome.bmp  164 x 314  (MUI welcome / finish side panel)
#   scripts\installer-header.bmp   150 x 57   (MUI header strip, top-right)
#
# Colors are the default Lunac palette:
#   background #16181B / surface #1B1E22 / accent #CFC8BB / muted #8B8E92
#
# The script is deterministic and only needs System.Drawing (always present on
# Windows PowerShell 5.1). Re-run it if the palette changes.

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$outDir = $PSScriptRoot
$bg = [System.Drawing.Color]::FromArgb(22, 24, 27)
$surface = [System.Drawing.Color]::FromArgb(27, 30, 34)
$accent = [System.Drawing.Color]::FromArgb(207, 200, 187)
$muted = [System.Drawing.Color]::FromArgb(139, 142, 146)

function New-LunacBitmap {
    param(
        [int]$Width,
        [int]$Height,
        [string]$Path,
        [scriptblock]$Draw
    )
    $bmp = New-Object System.Drawing.Bitmap(
        $Width,
        $Height,
        [System.Drawing.Imaging.PixelFormat]::Format24bppRgb
    )
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

    $rect = New-Object System.Drawing.Rectangle(0, 0, $Width, $Height)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $rect,
        $bg,
        $surface,
        90.0
    )
    $g.FillRectangle($brush, $rect)
    $brush.Dispose()

    & $Draw $g $Width $Height

    $g.Dispose()
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $bmp.Dispose()
    Write-Host "  $Path ($Width x $Height, 24bpp)"
}

function New-Brush([System.Drawing.Color]$Color) {
    return New-Object System.Drawing.SolidBrush($Color)
}

function New-Font([float]$Size, [System.Drawing.FontStyle]$Style) {
    return New-Object System.Drawing.Font(
        "Segoe UI",
        $Size,
        $Style,
        [System.Drawing.GraphicsUnit]::Point
    )
}

New-LunacBitmap -Width 164 -Height 314 -Path (Join-Path $outDir "installer-welcome.bmp") -Draw {
    param($g, $w, $h)
    $accentBrush = New-Brush $accent
    $mutedBrush = New-Brush $muted

    # Accent rule down the left edge.
    $g.FillRectangle($accentBrush, 18, 30, 2, $h - 60)

    $titleFont = New-Font 18 ([System.Drawing.FontStyle]::Bold)
    $tagFont = New-Font 8 ([System.Drawing.FontStyle]::Regular)
    $footFont = New-Font 7 ([System.Drawing.FontStyle]::Regular)

    $g.DrawString("LUNAC", $titleFont, $accentBrush, 30, 30)
    $g.DrawString("DESKTOP AI", $tagFont, $mutedBrush, 30, 62)
    $g.DrawString("LOCAL FIRST", $footFont, $mutedBrush, 30, $h - 48)

    # A quiet accent rule near the bottom for depth.
    $g.FillRectangle($accentBrush, 30, $h - 86, 18, 1)

    $titleFont.Dispose()
    $tagFont.Dispose()
    $footFont.Dispose()
    $accentBrush.Dispose()
    $mutedBrush.Dispose()
}

New-LunacBitmap -Width 150 -Height 57 -Path (Join-Path $outDir "installer-header.bmp") -Draw {
    param($g, $w, $h)
    $accentBrush = New-Brush $accent
    $mutedBrush = New-Brush $muted

    $g.FillRectangle($accentBrush, 10, 12, 2, 33)

    $titleFont = New-Font 12 ([System.Drawing.FontStyle]::Bold)
    $tagFont = New-Font 7 ([System.Drawing.FontStyle]::Regular)

    $g.DrawString("LUNAC", $titleFont, $accentBrush, 20, 10)
    $g.DrawString("SETUP", $tagFont, $mutedBrush, 20, 32)

    $titleFont.Dispose()
    $tagFont.Dispose()
    $accentBrush.Dispose()
    $mutedBrush.Dispose()
}

New-LunacBitmap -Width 55 -Height 55 -Path (Join-Path $outDir "installer-small.bmp") -Draw {
    param($g, $w, $h)
    $accentBrush = New-Brush $accent
    $mutedBrush = New-Brush $muted

    $g.FillRectangle($accentBrush, 12, 12, 2, 31)
    $titleFont = New-Font 15 ([System.Drawing.FontStyle]::Bold)
    $g.DrawString("L", $titleFont, $accentBrush, 20, 12)
    $titleFont.Dispose()
    $accentBrush.Dispose()
    $mutedBrush.Dispose()
}

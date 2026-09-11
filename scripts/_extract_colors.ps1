Add-Type -AssemblyName System.Drawing

# 从仓库根目录下的 twitter*.jpg 采样，统计均色与高频色（图标配色参考用）
$repoRoot = Split-Path -Parent $PSScriptRoot
$src = Get-ChildItem -Path $repoRoot -Filter '*.jpg' | Where-Object { $_.Name -like 'twitter*' } | Select-Object -First 1
if (-not $src) { Write-Output 'NO_JPG_FOUND'; exit 1 }

$tmp = Join-Path $env:TEMP 'lunac_sample.jpg'
Copy-Item -LiteralPath $src.FullName -Destination $tmp -Force

$img = [System.Drawing.Bitmap]::FromFile($tmp)
$sw = 48
$sh = 48
$bmp = New-Object System.Drawing.Bitmap $img, $sw, $sh

$sumR = 0.0; $sumG = 0.0; $sumB = 0.0; $n = 0
$hist = @{}
for ($y = 0; $y -lt $sh; $y++) {
  for ($x = 0; $x -lt $sw; $x++) {
    $c = $bmp.GetPixel($x, $y)
    $sumR += $c.R; $sumG += $c.G; $sumB += $c.B; $n++
    $r = [int]([math]::Floor($c.R / 24) * 24)
    $g = [int]([math]::Floor($c.G / 24) * 24)
    $b = [int]([math]::Floor($c.B / 24) * 24)
    $key = '{0},{1},{2}' -f $r, $g, $b
    if ($hist.ContainsKey($key)) { $hist[$key]++ } else { $hist[$key] = 1 }
  }
}

$avg = '#{0:X2}{1:X2}{2:X2}' -f [int]($sumR / $n), [int]($sumG / $n), [int]($sumB / $n)
Write-Output ("SIZE {0}x{1}" -f $img.Width, $img.Height)
Write-Output ("AVG  {0}" -f $avg)
Write-Output "TOP12 (hex -> count):"
$hist.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 12 | ForEach-Object {
  $rgb = $_.Key -split ','
  $hex = '#{0:X2}{1:X2}{2:X2}' -f [int]$rgb[0], [int]$rgb[1], [int]$rgb[2]
  Write-Output ("  {0}  count={1}" -f $hex, $_.Value)
}

$bmp.Dispose(); $img.Dispose()
Remove-Item -LiteralPath $tmp -Force

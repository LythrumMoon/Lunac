# Parse a Windows kernel minidump to extract the bugcheck (BSOD) code.
#
# 用法：  powershell -File docs\parse-minidump.ps1 -Path 'C:\Windows\Minidump\xxxx.dmp'
#         powershell -File docs\parse-minidump.ps1            # 不给就取该目录下最新的一个
#
# **不得把某台机器的 .dmp 文件名写死成默认值**（2026-09-19 改）：原来默认值是
# `C:\Windows\Minidump\080226-14562-01.dmp` —— 那是某次排查时本机的产物，
# 换台机器就必然 `ReadAllBytes` 抛异常（见 ai-spec：禁止硬编码本机绝对路径）。
param([string]$Path = "")

if (-not $Path) {
    $dir = Join-Path $env:SystemRoot "Minidump"
    $latest = Get-ChildItem -LiteralPath $dir -Filter *.dmp -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $latest) {
        Write-Error "没有找到 dump 文件（默认目录 $dir）。请用 -Path 显式指定。"
        exit 1
    }
    $Path = $latest.FullName
    Write-Host "未指定 -Path，取最新的 dump：$Path"
}

if (-not (Test-Path -LiteralPath $Path)) {
    Write-Error "dump 文件不存在：$Path"
    exit 1
}

$bytes = [System.IO.File]::ReadAllBytes($Path)
$br = New-Object System.IO.BinaryReader (New-Object System.IO.MemoryStream (,$bytes))

function ReadU32($o) { [BitConverter]::ToUInt32($bytes, $o) }
function ReadU64($o) { [BitConverter]::ToUInt64($bytes, $o) }

$sig = $br.BaseStream.Position
# 1. Verify MDMP signature (u32) + version (u32) = 0xA793
$signature = ReadU32(0)
$version   = ReadU32(4)
$numStreams = ReadU32(8)
$dirRva     = ReadU32(12)

Write-Host "Signature: 0x$('{0:X8}' -f $signature)  Version: 0x$('{0:X8}' -f $version)  Streams: $numStreams  DirRVA: $dirRva"

# 2. Walk the stream directory to find the Exception stream (type 6)
for ($i = 0; $i -lt $numStreams; $i++) {
    $off = $dirRva + $i * 12
    $type = ReadU32($off)
    $dataSize = ReadU32($off + 4)
    $dataRva  = ReadU32($off + 8)

    if ($type -eq 6) { # ExceptionStream
        Write-Host "Found ExceptionStream at RVA $dataRva size $dataSize"
        $ex = $dataRva
        $code = ReadU32($ex)            # ExceptionCode
        $flags = ReadU32($ex + 4)
        $addr = ReadU64($ex + 8)        # ExceptionAddress
        $numParams = ReadU32($ex + 24)  # NumberParameters
        Write-Host "ExceptionCode: 0x$('{0:X8}' -f $code)  Address: 0x$('{0:X}' -f $addr)  NumParams: $numParams"
        if ($numParams -gt 0) {
            $p0 = ReadU64($ex + 32)
            $p1 = ReadU64($ex + 40)
            $p2 = ReadU64($ex + 48)
            $p3 = ReadU64($ex + 56)
            if ($code -eq 0x80000003) {
                Write-Host "BUGCHECK: 0x$('{0:X8}' -f $p0)  param1=0x$('{0:X}' -f $p1) param2=0x$('{0:X}' -f $p2) param3=0x$('{0:X}' -f $p3)"
            } else {
                Write-Host "EXCEPTION: 0x$('{0:X8}' -f $code)  info0=0x$('{0:X}' -f $p0)"
            }
        }
        break
    }
}
$br.Close()

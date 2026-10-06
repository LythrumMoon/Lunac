# scripts/reconcile-usage.ps1 — 本地用量账 vs 供应商平台 CSV 逐小时对账（成本归因核对工具）
#
# 定位：这是「核对 / 归因」工具，不是产品功能；**只读**，不改仓库任何文件。
#
# 两份数据的口径：
#   · 本地账 app\src-tauri\target\debug\ModuleData\usage\usage-YYYY-MM-DD.jsonl
#     每行 = 一次提问，input/output/cacheRead/cacheCreate 都是**本次提问的绝对值**
#     （不是累计），行内自带 ts（UTC epoch 毫秒）；文件按本地日期分片。
#   · 平台 CSV 是另一套口径（可能按 UTC、可能带时区），两者只能「逐小时」对齐。
#
# 金额口径与仓库唯一实现 app\src\usage-cost.ts 的 priceAt()/dayCost() 完全一致：
#   ① 本地时刻 = ts + UtcOffsetMinutes（东八区 = 480）；
#   ② **逐小时桶计价**，绝不「总量 × 某一个价」；
#   ③ 模型未定价 ⇒ 该桶不算金额、标「未定价」、不计入合计（**不当 0**，不借别的模型单价）；
#   ④ **法定节假日全天按基础价（谷价）**：价目表顶层（或模型级覆盖）的 `holidays` 命中的
#      那一天，`time_windows`（高峰价）一律不适用 —— 官方脚注是「周一至周五**不含中国法定
#      节假日**才算高峰」。2026-10-02（国庆、周五）实测：漏了这条会把全天按峰价高估。
#
# 退出码：0 = 无差异；1 = 存在 DIFF；2 = 用法错误 / 文件读不到 / CSV 表头认不出。

[CmdletBinding()]
param(
    [string]$UsageDir,
    [string]$Pricing,
    [int]$UtcOffsetMinutes = 480,
    [string]$Csv
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $UsageDir) { $UsageDir = Join-Path $repoRoot "app\src-tauri\target\debug\ModuleData\usage" }
if (-not $Pricing)  { $Pricing  = Join-Path $repoRoot "app\src-tauri\target\debug\config\pricing.json" }

$Inv = [System.Globalization.CultureInfo]::InvariantCulture

# ---------------- 小工具 ----------------

function Read-Utf8Text([string]$path) {
    return [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
}
function Read-Utf8Lines([string]$path) {
    return [System.IO.File]::ReadAllLines($path, [System.Text.Encoding]::UTF8)
}
# 数值兜底：缺失按 0（与 usage-cost.ts 里 `p.input || 0` 同口径）
function To-Num($v) {
    if ($null -eq $v) { return [decimal]0 }
    return [decimal]$v
}
# 宽松数值解析：容忍千分位逗号与货币符号，只取第一段数字
function To-Decimal([string]$s) {
    if ([string]::IsNullOrWhiteSpace($s)) { return [decimal]0 }
    $t = $s.Trim().Replace(',', '')
    $m = [regex]::Match($t, '-?\d+(\.\d+)?')
    if (-not $m.Success) { return [decimal]0 }
    return [decimal]::Parse($m.Value, $Inv)
}
# `HH:MM` → 零点起的分钟数；写法不合法返回 $null（严格口径，同 usage-cost.ts 的 hhmmToMinutes）
function Convert-Hhmm-ToMinutes([string]$s) {
    if ($null -eq $s) { return $null }
    $m = [regex]::Match($s, '^(\d{2}):(\d{2})$')
    if (-not $m.Success) { return $null }
    $h = [int]$m.Groups[1].Value
    $mi = [int]$m.Groups[2].Value
    if ($h -gt 23 -or $mi -gt 59) { return $null }
    return $h * 60 + $mi
}
# `YYYY-MM-DD` → ISO 周几（1=周一…7=周日）；解析不出返回 0
function Get-IsoWeekday([string]$date) {
    $m = [regex]::Match($date, '^(\d{4})-(\d{2})-(\d{2})$')
    if (-not $m.Success) { return 0 }
    $d = New-Object System.DateTime([int]$m.Groups[1].Value, [int]$m.Groups[2].Value, [int]$m.Groups[3].Value)
    return ((([int]$d.DayOfWeek) + 6) % 7) + 1
}
# ts（UTC epoch 毫秒）→ 本地 DateTime
function Convert-TsToLocal([double]$ts) {
    $ms = [long][math]::Round($ts)
    $utc = [System.DateTimeOffset]::FromUnixTimeMilliseconds($ms).UtcDateTime
    return $utc.AddMinutes($UtcOffsetMinutes)
}
# 价格表里取某模型条目（取不到 = 未定价）
function Get-ModelEntry($pricing, [string]$model) {
    if ($null -eq $pricing) { return $null }
    $models = $pricing.models
    if ($null -eq $models) { return $null }
    if ($null -eq $model) { $model = "" }
    $prop = $models.PSObject.Properties[$model]
    if ($null -eq $prop) { return $null }
    return $prop.Value
}
# 某模型可用到的**法定节假日集合**（`YYYY-MM-DD`）：模型级 `holidays` 优先，缺省回落顶层。
# 命中 ⇒ 当天**全天按基础价（谷价）**，时段窗一律不适用（官方脚注：周一至周五**不含中国
# 法定节假日**才算高峰；2026-10-02 国庆实测就是全天谷价）。返回 Hashtable 便于 O(1) 查。
function Get-HolidaySet($pricing, $entry) {
    $src = $null
    if ($null -ne $entry) { $src = $entry.holidays }
    if ($null -eq $src -and $null -ne $pricing) { $src = $pricing.holidays }
    $h = @{}
    if ($null -ne $src) {
        foreach ($d in @($src)) { if ($null -ne $d) { $h[[string]$d] = $true } }
    }
    return $h
}
# 该模型在「本地日期 + 本地小时」这一格实际生效的价（分时价：第一条命中覆盖基础价）。
# **法定节假日全天直接回基础价**（见 Get-HolidaySet）。未定价返回 $null；窗口只认整点起点
# （与本仓库 usage-cost.ts 的 priceAt 一致 —— 两边是同一套口径，改一处必须改另一处）。
function Get-PriceAt($pricing, [string]$model, [string]$date, [int]$hour) {
    $entry = Get-ModelEntry $pricing $model
    if ($null -eq $entry) { return $null }
    $wins = @($entry.time_windows)
    $hols = Get-HolidaySet $pricing $entry
    if ($wins.Count -gt 0 -and -not $hols.ContainsKey($date)) {
        $weekday = Get-IsoWeekday $date
        $mins = $hour * 60
        foreach ($w in $wins) {
            if ($null -eq $w) { continue }
            $from = Convert-Hhmm-ToMinutes ([string]$w.from)
            $to = Convert-Hhmm-ToMinutes ([string]$w.to)
            if ($null -eq $from -or $null -eq $to -or $from -ge $to) { continue }
            if ($mins -lt $from -or $mins -ge $to) { continue }
            $days = @($w.days)
            if ($days.Count -gt 0 -and ($days -notcontains $weekday)) { continue }
            return @{
                src    = ("{0}-{1}" -f ([string]$w.from), ([string]$w.to))
                input  = To-Num $w.input
                read   = To-Num $w.cache_read
                write  = To-Num $w.cache_write
                output = To-Num $w.output
            }
        }
    }
    return @{
        src    = "base"
        input  = To-Num $entry.input
        read   = To-Num $entry.cache_read
        write  = To-Num $entry.cache_write
        output = To-Num $entry.output
    }
}
# 单价是「元 / 百万 token」，故末尾 /1e6。参数名不能叫 $input（PowerShell 自动变量，会编译报错）。
function Calc-Amount($price, [decimal]$inTok, [decimal]$cRead, [decimal]$cCreate, [decimal]$outTok) {
    return ($inTok * $price.input + $cRead * $price.read + $cCreate * $price.write + $outTok * $price.output) / [decimal]1000000
}
# 极简 CSV 行切分（支持双引号包裹与 "" 转义）
function Split-CsvLine([string]$line) {
    $result = New-Object System.Collections.ArrayList
    $sb = New-Object System.Text.StringBuilder
    $inQuotes = $false
    for ($i = 0; $i -lt $line.Length; $i++) {
        $c = $line[$i]
        if ($inQuotes) {
            if ($c -eq '"') {
                if ($i + 1 -lt $line.Length -and $line[$i + 1] -eq '"') { [void]$sb.Append('"'); $i++ }
                else { $inQuotes = $false }
            } else { [void]$sb.Append($c) }
        } else {
            if ($c -eq '"') { $inQuotes = $true }
            elseif ($c -eq ',') { [void]$result.Add($sb.ToString()); [void]$sb.Clear() }
            else { [void]$sb.Append($c) }
        }
    }
    [void]$result.Add($sb.ToString())
    # 直接返回扁平数组（调用处用 @() 收集）；不要写 `return ,$arr`，
    # 否则函数把整个数组当「单个对象」输出，再被 @() 包一层就成了嵌套数组。
    return $result.ToArray()
}
# 在归一化表头里按关键字找列下标（大小写去空格下划线后 contains 匹配）
function Find-Col([string[]]$norm, [string[]]$keys) {
    for ($i = 0; $i -lt $norm.Count; $i++) {
        foreach ($k in $keys) { if ($norm[$i].Contains($k)) { return $i } }
    }
    return -1
}
# CSV 时间字符串 → @{ date; hour } 本地键；解析失败返回 $null
function Get-LocalHourKey([string]$s) {
    if ([string]::IsNullOrWhiteSpace($s)) { return $null }
    $t = $s.Trim().Trim('"')
    try {
        if ($t -match '(?i)(Z|[+-]\d{2}:?\d{2})\s*$') {
            # 带时区标记：先归一到 UTC，再按 UtcOffsetMinutes 换算本地
            $dto = [System.DateTimeOffset]::Parse($t, $Inv)
            $local = $dto.UtcDateTime.AddMinutes($UtcOffsetMinutes)
        } else {
            # 无时区标记：直接当本地时刻
            $local = [datetime]::Parse($t, $Inv, [System.Globalization.DateTimeStyles]::None)
        }
    } catch { return $null }
    return @{ date = $local.ToString('yyyy-MM-dd'); hour = $local.Hour }
}

# ---------------- 读价格表 ----------------

if (-not (Test-Path -LiteralPath $Pricing)) {
    Write-Host ("ERROR: 价格表读不到: {0}" -f $Pricing) -ForegroundColor Red
    exit 2
}
# 注意：$pricingFile 不能叫 $pricing —— PowerShell 变量名大小写不敏感，
# 会和参数 $Pricing（[string] 受约束）撞名，赋值时把路径覆盖掉。
$pricingFile = $null
try { $pricingFile = (Read-Utf8Text $Pricing) | ConvertFrom-Json } catch { $pricingFile = $null }
if ($null -eq $pricingFile) {
    Write-Host ("ERROR: 价格表不是合法 JSON: {0}" -f $Pricing) -ForegroundColor Red
    exit 2
}

# ---------------- 读本地用量账，按「本地日期 + 本地小时 + 模型」聚合 ----------------

if (-not (Test-Path -LiteralPath $UsageDir)) {
    Write-Host ("ERROR: 用量目录不存在: {0}" -f $UsageDir) -ForegroundColor Red
    exit 2
}
$files = @(Get-ChildItem -LiteralPath $UsageDir -Filter 'usage-*.jsonl' -File)
if ($files.Count -eq 0) {
    Write-Host ("警告: 用量目录下没有 usage-*.jsonl: {0}" -f $UsageDir) -ForegroundColor Yellow
}

$localBuckets = @{}   # "date|hour|model" -> @{ date; hour; model; input; cRead; cCreate; output; turns }
$badLines = 0
foreach ($f in $files) {
    foreach ($ln in (Read-Utf8Lines $f.FullName)) {
        if ([string]::IsNullOrWhiteSpace($ln)) { continue }
        $rec = $null
        try { $rec = $ln | ConvertFrom-Json } catch { $badLines++; continue }
        if ($null -eq $rec -or $null -eq $rec.ts) { $badLines++; continue }
        $lt = Convert-TsToLocal ([double]$rec.ts)
        $date = $lt.ToString('yyyy-MM-dd')
        $hour = $lt.Hour
        $model = [string]$rec.model
        $key = "{0}|{1}|{2}" -f $date, $hour, $model
        if (-not $localBuckets.ContainsKey($key)) {
            $localBuckets[$key] = @{
                date = $date; hour = $hour; model = $model
                input = [decimal]0; cRead = [decimal]0; cCreate = [decimal]0; output = [decimal]0
                turns = 0
            }
        }
        $b = $localBuckets[$key]
        $b.input   += To-Num $rec.input
        $b.cRead   += To-Num $rec.cacheRead
        $b.cCreate += To-Num $rec.cacheCreate
        $b.output  += To-Num $rec.output
        $b.turns   += 1
    }
}
if ($badLines -gt 0) {
    Write-Host ("警告: 有 {0} 行无法解析，已跳过。" -f $badLines) -ForegroundColor Yellow
}

$sorted = @($localBuckets.Values | Sort-Object date, hour, model)
foreach ($b in $sorted) {
    # 传**日期**而不是周几：法定节假日要按「具体哪一天」判（见 Get-PriceAt）
    $p = Get-PriceAt $pricingFile $b.model $b.date $b.hour
    if ($null -eq $p) { $b.amt = $null; $b.src = "未定价" }
    else { $b.amt = Calc-Amount $p $b.input $b.cRead $b.cCreate $b.output; $b.src = $p.src }
}

# ---------------- 打印本地逐小时表 ----------------

Write-Host ""
Write-Host "=== 本地用量账（逐本地小时） ===" -ForegroundColor Cyan
Write-Host ("usage dir: {0}" -f $UsageDir)
Write-Host ("pricing  : {0}" -f $Pricing)
Write-Host ("utc offset minutes: {0}" -f $UtcOffsetMinutes)
Write-Host ""

$out = New-Object System.Text.StringBuilder
[void]$out.AppendLine("date        hr  model                 in        read      create    out       turns  amount          price")
foreach ($b in $sorted) {
    $modelDisp = $b.model
    if ([string]::IsNullOrEmpty($modelDisp)) { $modelDisp = "(none)" }
    if ($null -eq $b.amt) { $amtText = "未定价" } else { $amtText = $b.amt.ToString('F8', $Inv) }
    [void]$out.AppendLine((
        "{0}  {1,2}  {2,-20}  {3,8}  {4,8}  {5,8}  {6,8}  {7,5}  {8,-15} {9}" -f `
            $b.date, $b.hour, $modelDisp,
            $b.input.ToString('0', $Inv), $b.cRead.ToString('0', $Inv),
            $b.cCreate.ToString('0', $Inv), $b.output.ToString('0', $Inv),
            $b.turns, $amtText, $b.src
    ))
}
# 合计行
$tIn = [decimal]0; $tRead = [decimal]0; $tCreate = [decimal]0; $tOut = [decimal]0; $tTurns = 0; $tAmt = [decimal]0
$unpriced = @{}
foreach ($b in $sorted) {
    $tIn += $b.input; $tRead += $b.cRead; $tCreate += $b.cCreate; $tOut += $b.output; $tTurns += $b.turns
    if ($null -eq $b.amt) { if (-not $unpriced.ContainsKey($b.model)) { $unpriced[$b.model] = $true } }
    else { $tAmt += $b.amt }
}
[void]$out.AppendLine((
    "{0}  {1,2}  {2,-20}  {3,8}  {4,8}  {5,8}  {6,8}  {7,5}  {8,-15} {9}" -f `
        "TOTAL", "", "", $tIn.ToString('0', $Inv), $tRead.ToString('0', $Inv),
        $tCreate.ToString('0', $Inv), $tOut.ToString('0', $Inv), $tTurns,
        $tAmt.ToString('F6', $Inv), ""
))
Write-Host $out.ToString()
if ($unpriced.Count -gt 0) {
    $names = @($unpriced.Keys | ForEach-Object { if ([string]::IsNullOrEmpty($_)) { "(none)" } else { $_ } })
    Write-Host ("  未定价模型（金额未计入合计）: {0}" -f ($names -join ", ")) -ForegroundColor Yellow
}
Write-Host ("  合计金额（仅已定价部分，6 位小数）: {0}" -f $tAmt.ToString('F6', $Inv)) -ForegroundColor Green

# 精度声明（如实打印，不假装精确到分钟）
Write-Host ""
Write-Host "精度声明：本地账每行只带**开始时刻 ts**，因此一次提问若跨越小时边界，会被**整条**算进开始那一小时。" -ForegroundColor Yellow
Write-Host "          这是本地账的精度上限（桶粒度 = 整点）；跨小时的长提问在本表里会有 ≤1 小时的归属偏移。" -ForegroundColor Yellow
Write-Host ""

# ---------------- 无 CSV：到此为止 ----------------

if (-not $Csv) {
    Write-Host "（未提供 -Csv，仅输出本地逐小时账。）" -ForegroundColor DarkGray
    exit 0
}

# ---------------- CSV 对照 ----------------

if (-not (Test-Path -LiteralPath $Csv)) {
    Write-Host ("ERROR: CSV 读不到: {0}" -f $Csv) -ForegroundColor Red
    exit 2
}
$csvLines = @(Read-Utf8Lines $Csv)
$header = $null; $startIdx = 0
for ($i = 0; $i -lt $csvLines.Count; $i++) {
    if (-not [string]::IsNullOrWhiteSpace($csvLines[$i])) { $header = $csvLines[$i]; $startIdx = $i + 1; break }
}
if ($null -eq $header) {
    Write-Host ("ERROR: CSV 是空的: {0}" -f $Csv) -ForegroundColor Red
    exit 2
}
$cols = @(Split-CsvLine $header)
if ($cols.Count -gt 0) { $cols[0] = $cols[0].TrimStart([char]0xFEFF) }
$norm = @()
foreach ($c in $cols) { $norm += ($c -replace '[\s_\u3000]', '').ToLowerInvariant() }

$ti = Find-Col $norm @('time', 'date', '时间')
$ai = Find-Col $norm @('amount', 'cost', '金额', '费用')
if ($ti -lt 0 -or $ai -lt 0) {
    Write-Host "ERROR: 无法识别 CSV 表头（时间列与金额列为必需）。实际表头如下：" -ForegroundColor Red
    Write-Host $header
    exit 2
}
$ii = Find-Col $norm @('input', 'prompt', '输入')
$oi = Find-Col $norm @('output', 'completion', '输出')
$ri = Find-Col $norm @('cacheread', 'hit', '缓存命中')
$wi = Find-Col $norm @('cachewrite', 'create', '缓存写')

# 平台侧按「本地日期 + 本地小时」聚合
$plat = @{}
for ($r = $startIdx; $r -lt $csvLines.Count; $r++) {
    $ln = $csvLines[$r]
    if ([string]::IsNullOrWhiteSpace($ln)) { continue }
    $fld = @(Split-CsvLine $ln)
    if ($ti -ge $fld.Count) { continue }
    $tk = Get-LocalHourKey ([string]$fld[$ti])
    if ($null -eq $tk) { continue }
    $key = "{0}|{1}" -f $tk.date, $tk.hour
    if (-not $plat.ContainsKey($key)) {
        $plat[$key] = @{ date = $tk.date; hour = $tk.hour; input = [decimal]0; cRead = [decimal]0; cCreate = [decimal]0; output = [decimal]0; amount = [decimal]0 }
    }
    $pb = $plat[$key]
    if ($ii -ge 0 -and $ii -lt $fld.Count) { $pb.input   += To-Decimal ([string]$fld[$ii]) }
    if ($ri -ge 0 -and $ri -lt $fld.Count) { $pb.cRead   += To-Decimal ([string]$fld[$ri]) }
    if ($wi -ge 0 -and $wi -lt $fld.Count) { $pb.cCreate += To-Decimal ([string]$fld[$wi]) }
    if ($oi -ge 0 -and $oi -lt $fld.Count) { $pb.output  += To-Decimal ([string]$fld[$oi]) }
    if ($ai -ge 0 -and $ai -lt $fld.Count) { $pb.amount  += To-Decimal ([string]$fld[$ai]) }
}

# 本地侧再折叠成「本地日期 + 本地小时」（跨模型求和）；金额仍逐桶计价
$localHour = @{}
foreach ($b in $sorted) {
    $key = "{0}|{1}" -f $b.date, $b.hour
    if (-not $localHour.ContainsKey($key)) {
        $localHour[$key] = @{ date = $b.date; hour = $b.hour; input = [decimal]0; cRead = [decimal]0; cCreate = [decimal]0; output = [decimal]0; amount = [decimal]0 }
    }
    $lb = $localHour[$key]
    $lb.input += $b.input; $lb.cRead += $b.cRead; $lb.cCreate += $b.cCreate; $lb.output += $b.output
    if ($null -ne $b.amt) { $lb.amount += $b.amt }
}

$keys = @(@($localHour.Keys) + @($plat.Keys) | Sort-Object -Unique)
Write-Host "=== 逐小时对照：本地 vs 平台 ===" -ForegroundColor Cyan
Write-Host ("csv: {0}" -f $Csv)
Write-Host ("识别列：time=#{0} amount=#{1} input=#{2} output=#{3} cache_read=#{4} cache_write=#{5}" -f $ti, $ai, $ii, $oi, $ri, $wi) -ForegroundColor DarkGray
Write-Host ""

$zero = @{ input = [decimal]0; cRead = [decimal]0; cCreate = [decimal]0; output = [decimal]0; amount = [decimal]0 }
$diffCount = 0
$lT = @{ input = [decimal]0; cRead = [decimal]0; cCreate = [decimal]0; output = [decimal]0; amount = [decimal]0 }
$pT = @{ input = [decimal]0; cRead = [decimal]0; cCreate = [decimal]0; output = [decimal]0; amount = [decimal]0 }
# 手写定宽表（Format-Table 会按控制台宽度裁掉右侧列，差额/DIFF 列就看不见了）
$fmt = "{0} {1,2}  {2,8} {3,8} {4,8} {5,8} {6,10}  {7,8} {8,8} {9,8} {10,8} {11,10}  {12,8} {13,8} {14,8} {15,8} {16,10}  {17}"
Write-Host ($fmt -f "date", "hr", "L.in", "L.read", "L.create", "L.out", "L.amt", "P.in", "P.read", "P.create", "P.out", "P.amt", "dIn", "dRead", "dCreate", "dOut", "dAmt", "flag")
foreach ($k in $keys) {
    if ($localHour.ContainsKey($k)) { $L = $localHour[$k] } else { $L = $zero }
    if ($plat.ContainsKey($k)) { $P = $plat[$k] } else { $P = $zero }
    $dAmt = $L.amount - $P.amount
    $flag = ''
    if ([math]::Abs($dAmt) -gt [decimal]0.0001) { $flag = 'DIFF'; $diffCount++ }
    $lT.input += $L.input; $lT.cRead += $L.cRead; $lT.cCreate += $L.cCreate; $lT.output += $L.output; $lT.amount += $L.amount
    $pT.input += $P.input; $pT.cRead += $P.cRead; $pT.cCreate += $P.cCreate; $pT.output += $P.output; $pT.amount += $P.amount
    $dateVal = $L.date; if ($null -eq $dateVal) { $dateVal = $P.date }
    $hourVal = $L.hour; if ($null -eq $hourVal) { $hourVal = $P.hour }
    $line = $fmt -f `
        $dateVal, $hourVal, `
        $L.input.ToString('0', $Inv), $L.cRead.ToString('0', $Inv), $L.cCreate.ToString('0', $Inv), $L.output.ToString('0', $Inv), $L.amount.ToString('F6', $Inv), `
        $P.input.ToString('0', $Inv), $P.cRead.ToString('0', $Inv), $P.cCreate.ToString('0', $Inv), $P.output.ToString('0', $Inv), $P.amount.ToString('F6', $Inv), `
        ($L.input - $P.input).ToString('0', $Inv), ($L.cRead - $P.cRead).ToString('0', $Inv), ($L.cCreate - $P.cCreate).ToString('0', $Inv), ($L.output - $P.output).ToString('0', $Inv), $dAmt.ToString('F6', $Inv), `
        $flag
    if ($flag) { Write-Host $line -ForegroundColor Red } else { Write-Host $line }
}
if ($keys.Count -eq 0) { Write-Host "（两侧都没有可对照的小时。）" -ForegroundColor Yellow }

Write-Host ("本地合计: in={0} read={1} create={2} out={3} amount={4}" -f `
    $lT.input.ToString('0', $Inv), $lT.cRead.ToString('0', $Inv), $lT.cCreate.ToString('0', $Inv), $lT.output.ToString('0', $Inv), $lT.amount.ToString('F6', $Inv))
Write-Host ("平台合计: in={0} read={1} create={2} out={3} amount={4}" -f `
    $pT.input.ToString('0', $Inv), $pT.cRead.ToString('0', $Inv), $pT.cCreate.ToString('0', $Inv), $pT.output.ToString('0', $Inv), $pT.amount.ToString('F6', $Inv))
Write-Host ("总差额  : in={0} read={1} create={2} out={3} amount={4}" -f `
    ($lT.input - $pT.input).ToString('0', $Inv), ($lT.cRead - $pT.cRead).ToString('0', $Inv), ($lT.cCreate - $pT.cCreate).ToString('0', $Inv), `
    ($lT.output - $pT.output).ToString('0', $Inv), ($lT.amount - $pT.amount).ToString('F6', $Inv))
Write-Host ""

if ($diffCount -gt 0) {
    Write-Host ("存在 {0} 个 DIFF 小时（金额差绝对值 > 0.0001 元）。" -f $diffCount) -ForegroundColor Red
    exit 1
}
Write-Host "无差异（逐小时金额全部在 0.0001 元容差内）。" -ForegroundColor Green
exit 0

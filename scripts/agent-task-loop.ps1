# scripts/agent-task-loop.ps1
# S2 长任务托管的「调度层最小版」（2026-10-22）。
#
# 做什么：反复调用 agent 的单发模式，让**检查点续跑**把同一个任务跨进程做完。
#   · 第 1 轮：`agent --print "<任务>" --checkpoint`；
#   · 之后每轮：`agent --print --continue`（恢复最近检查点 + 内置「接着做」提问）；
#   · 停止条件：退出码 0（清单完成）/ 其它非 0（失败）/ 达到 -MaxRounds。
#
# 退出码契约（见 core-agent/src/main.rs 的 single_shot_exit_code）：
#   0 = 完成；1 = 失败；3 = 正常结束但 TodoWrite 清单还有 pending / in_progress
#   （只有带检查点时才会出现 3；不带检查点的单发仍是 0/1）。
#
# 边界如实记：
#   · 调度就是一个**前台循环**，不是常驻服务；关掉终端就停。
#   · 每轮仍是一次独立进程 + 一次完整 API 回合；轮数由 -MaxRounds 封顶，避免无人值守时无限花钱。
#   · 默认 -PermissionMode plan（只读）。要写文件必须显式 -SkipPermissions ——
#     单发没有审批回包通道，写类工具只能靠这个开关放行（它有工作区风险，见 docs/forbidden.md §禁用 1.2）。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\agent-task-loop.ps1 -Query "把 README 里的 TODO 逐条做完"
#   powershell -ExecutionPolicy Bypass -File scripts\agent-task-loop.ps1 -Query "..." -MaxRounds 8 -PermissionMode acceptEdits -SkipPermissions

param(
    [Parameter(Mandatory = $true)]
    [string]$Query,
    [int]$MaxRounds = 5,
    [ValidateSet("plan", "acceptEdits")]
    [string]$PermissionMode = "plan",
    [switch]$SkipPermissions,
    [string]$WorkspaceDir,
    [string]$CheckpointDir,
    [string]$LogDir,
    [string]$Model,
    [string]$Token,
    [string]$BaseUrl,
    [string]$EnvFile,
    [switch]$Dev
)

$ErrorActionPreference = "Stop"

function Fail-Setup([string]$msg) {
    Write-Host ("[task-loop] ERROR: {0}" -f $msg) -ForegroundColor Red
    exit 2
}

function Read-Utf8Lines([string]$path) {
    return [System.IO.File]::ReadAllLines($path, [System.Text.Encoding]::UTF8)
}

# 解析 .env（KEY=VALUE，忽略 # 与空行，去包裹引号）
function Get-DotEnvMap([string]$path) {
    $map = @{}
    if (-not (Test-Path -LiteralPath $path)) { return $map }
    foreach ($ln in (Read-Utf8Lines $path)) {
        $t = $ln.Trim()
        if ($t -eq "" -or $t.StartsWith("#")) { continue }
        $eq = $t.IndexOf("=")
        if ($eq -lt 1) { continue }
        $k = $t.Substring(0, $eq).Trim()
        $v = $t.Substring($eq + 1).Trim()
        if ($v.Length -ge 2) {
            $first = $v.Substring(0, 1)
            $last = $v.Substring($v.Length - 1, 1)
            if (($first -eq '"' -and $last -eq '"') -or ($first -eq "'" -and $last -eq "'")) {
                $v = $v.Substring(1, $v.Length - 2)
            }
        }
        $map[$k] = $v
    }
    return $map
}

function First-NonEmpty([object[]]$values) {
    foreach ($v in $values) {
        if ($null -ne $v) {
            $s = [string]$v
            if ($s.Trim() -ne "") { return $s.Trim() }
        }
    }
    return $null
}

# 与宿主 commands.rs 的 agent_endpoint 同规则：有 AI_AGENT_URL 用它；否则剥末尾 /v1 再拼 /anthropic。
function Build-AgentEndpoint([string]$apiUrl, [string]$explicit) {
    if ($explicit -and $explicit.Trim() -ne "") { return $explicit.Trim() }
    $base = $apiUrl.Trim().TrimEnd("/")
    $root = $base
    if ($base.EndsWith("/v1")) {
        $r = $base.Substring(0, $base.Length - 3)
        if ($r.Contains("://")) { $root = $r }
    }
    return ($root + "/anthropic")
}

if ($MaxRounds -lt 1) { Fail-Setup "-MaxRounds 至少为 1" }
if ($WorkspaceDir) {
    if (-not (Test-Path -LiteralPath $WorkspaceDir -PathType Container)) {
        Fail-Setup ("-WorkspaceDir 不存在或不是目录: {0}" -f $WorkspaceDir)
    }
    $WorkspaceDir = (Resolve-Path -LiteralPath $WorkspaceDir).Path
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $EnvFile) { $EnvFile = Join-Path $repoRoot "app\src-tauri\.env" }
if (-not $CheckpointDir) { $CheckpointDir = Join-Path $repoRoot "temp\task-loop\sessions" }
if (-not $LogDir) { $LogDir = Join-Path $repoRoot "temp\task-loop\logs" }
[void](New-Item -ItemType Directory -Force -Path $CheckpointDir)
[void](New-Item -ItemType Directory -Force -Path $LogDir)

$exe = if ($Dev) {
    Join-Path $repoRoot "core-agent\target\debug\agent.exe"
} else {
    Join-Path $repoRoot "core-agent\target\release\agent.exe"
}
if (-not (Test-Path -LiteralPath $exe)) {
    $hint = if ($Dev) { "cargo build（cwd=core-agent）" } else { "cargo build --release（cwd=core-agent）" }
    Fail-Setup ("agent.exe 不存在: {0}；先 {1}（-Dev 才是 debug）" -f $exe, $hint)
}

# ── 凭据（与 agent-e2e.ps1 同一套解析顺序）────────────────────────
$envMap = Get-DotEnvMap $EnvFile
$aiJson = $null
$aiPath = Join-Path $repoRoot "app\src-tauri\config\ai.json"
if (Test-Path -LiteralPath $aiPath) {
    try { $aiJson = (Get-Content -LiteralPath $aiPath -Raw -Encoding UTF8) | ConvertFrom-Json } catch { $aiJson = $null }
}
$apiUrl = First-NonEmpty @(
    $BaseUrl,
    $env:AI_API_URL, $env:DEEPSEEK_URL,
    $(if ($aiJson) { $aiJson.url }),
    $envMap["AI_API_URL"], $envMap["DEEPSEEK_URL"],
    "https://api.deepseek.com"
)
$token = First-NonEmpty @(
    $Token,
    $env:AI_API_KEY, $env:DEEPSEEK_API_KEY,
    $(if ($aiJson) { $aiJson.key }),
    $envMap["AI_API_KEY"], $envMap["DEEPSEEK_API_KEY"]
)
$model = First-NonEmpty @(
    $Model,
    $env:AI_MODEL,
    $(if ($aiJson) { $aiJson.model }),
    $envMap["AI_MODEL"],
    "deepseek-flash"
)
$agentUrlExplicit = First-NonEmpty @($env:AI_AGENT_URL, $(if ($aiJson) { $aiJson.agent_url }), $envMap["AI_AGENT_URL"])
$endpoint = Build-AgentEndpoint $apiUrl $agentUrlExplicit
if (-not $token) { Fail-Setup ("没有可用的 API key：请配好 {0} 的 AI_API_KEY，或设 env / 传 -Token" -f $EnvFile) }

$env:LUNAC_AGENT_BASE_URL = $endpoint
$env:LUNAC_AGENT_TOKEN = $token
$env:LUNAC_AGENT_MODEL = $model
$env:LUNAC_LOG_DIR = $LogDir
$env:LUNAC_CHECKPOINT_DIR = $CheckpointDir

Write-Host ("[task-loop] exe={0}" -f $exe)
Write-Host ("[task-loop] checkpoint={0}" -f $CheckpointDir)
Write-Host ("[task-loop] logs={0}" -f $LogDir)
Write-Host ("[task-loop] maxRounds={0} permission={1} skipPermissions={2}" -f $MaxRounds, $PermissionMode, [bool]$SkipPermissions)

function Invoke-AgentRound([string[]]$extra) {
    # 注意：不要用 `$args`（PowerShell 自动变量），否则会与函数自身的未绑定参数混淆。
    $cliArgs = @("--print") + $extra + @("--output-format", "text", "--permission-mode", $PermissionMode)
    if ($WorkspaceDir) { $cliArgs += @("--add-dir", $WorkspaceDir) }
    if ($SkipPermissions) { $cliArgs += "--dangerously-skip-permissions" }
    # 必须 `| Out-Host`：否则 agent 的 stdout 会成为本函数的返回值（和退出码一起变成
    # `System.Object[]`，后面 `-eq 3` 的判断就永远不可靠）。
    # `-WorkspaceDir` 同时作为 agent 的 cwd（与 agent-e2e.ps1 的 WorkingDirectory 同义）：
    # 只加 `--add-dir` 的话，模型看到的环境块仍是调用者的 cwd，相对路径会对不上。
    Push-Location
    try {
        if ($WorkspaceDir) { Set-Location -LiteralPath $WorkspaceDir }
        & $exe @cliArgs | Out-Host
        return $LASTEXITCODE
    } finally {
        Pop-Location
    }
}

$round = 1
Write-Host ("[task-loop] === round {0}/{1} (first) ===" -f $round, $MaxRounds)
$code = Invoke-AgentRound @($Query, "--checkpoint")
Write-Host ("[task-loop] round {0} exit={1}" -f $round, $code)

while ($code -eq 3 -and $round -lt $MaxRounds) {
    $round++
    Write-Host ("[task-loop] === round {0}/{1} (--continue) ===" -f $round, $MaxRounds)
    $code = Invoke-AgentRound @("--continue")
    Write-Host ("[task-loop] round {0} exit={1}" -f $round, $code)
}

if ($code -eq 3) {
    Write-Host ("[task-loop] 达到 -MaxRounds（{0}）仍未完成；检查点保留在 {1}，可再跑 --continue" -f $MaxRounds, $CheckpointDir) -ForegroundColor Yellow
} elseif ($code -eq 0) {
    Write-Host ("[task-loop] 完成（{0} 轮）" -f $round) -ForegroundColor Green
} else {
    Write-Host ("[task-loop] 失败 exit={0}（{1} 轮）；日志见 {2}" -f $code, $round, $LogDir) -ForegroundColor Red
}
exit $code

# scripts/agent-e2e.ps1 — core-agent 的无头端到端实测脚本（走真实 API）
#
# 定位：AI 侧的可脚本化实测手段。直接拉起 core-agent 的 agent.exe，喂一条或多条 query，
# 捕获完整 stream-json 事件流，产出结构化结果与「预期效果」断言。**只读运行**（不改仓库、
# 不写 ModuleData），全部落盘落在 temp\agent-e2e 下的独立 run 目录里。
#
# 目标产物（**默认瞄准 release**，交付目标 = 用户实际安装 / 运行的版本）：
#   默认待测二进制 = core-agent\target\release\agent.exe；配套的 lunac 数据根也取同一 release 布局
#   （app\src-tauri\target\release）。dev 只是**显式例外**：加 -Dev 才指回
#   core-agent\target\debug\agent.exe。二选一，缺哪个就明确报错、以 exit 2 失败，**绝不静默回落**
#   （静默回落正是本纪律要纠正的毛病）。
#   开关名用 -Dev 而非 -Debug 的原因：本脚本带 [CmdletBinding()]，它自带的公共参数已占 -Debug，
#   再声明同名参数会抛「参数名 'Debug' 被多次定义」；-Dev 亦更贴合「指向 dev 产物」的语义。
#   宿主可执行文件（仅在 -McpExe 接桥时用到）：release = app\src-tauri\target\release\lunac.exe，
#   dev = app\src-tauri\target\debug\lunac.exe，安装版 = 安装根目录（如 $INSTDIR\lunac.exe）。
#
# 两条路径（并存，按场景选）：
#   1. 常驻 stdio（默认）：agent.exe 是常驻 stdio 循环，stdin 每行一条 JSON
#      （{"type":"user",...}），stdout 每行一条 JSON 事件；脚本把 user 行写进 stdin、关 stdin、
#      等退出。能给**完整事件流 + 工具序列 + 逐请求 usage**，适合多问 / 断工具往返 / 混测。
#   2. 单发 --print（加 -Print）：agent.exe 的 --print 单发模式，一次性跑恰好一轮提问后退出；
#      -Print 用 --print --output-format json，stdin 写 query 原文，stdout 恰好一行 result JSON。
#      更简单直接，只给**最终文本 + usage + 成败**（json 输出不含工具序列事件，故 -ExpectTool 不适用）。
#
# 凭据：不落明文 key 进脚本 / 日志 / 输出。解析顺序见 Resolve-Creds（参数 > 进程 env >
#   <config>\ai.json > app\src-tauri\.env > 内置缺省）。任何进屏幕 / 落盘的文本都过 Protect-Text。
#
# 用法（示例）：
#   # 冒烟（常驻 stdio）：默认走 release\agent.exe，问一句，断言最终文本含 PONG
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Query "Reply with the single word: PONG" -ExpectContains PONG
#
#   # 冒烟（单发 --print）：更简单的路径，只要最终文本
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Print -Query "Reply with the single word: PONG" -ExpectContains PONG
#
#   # 显式改用 dev 构建（例外）：指回 target\debug\agent.exe
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Dev -Query "Reply with the single word: PONG" -ExpectContains PONG
#
#   # 多问 + 断言工具序列里有某个工具（只读档下 Read/Glob/Grep 免审批）
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -QueryFile q.txt -ExpectTool Read
#
#   # 覆盖模型 / 端点（不改用户配置）
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Model deepseek-flash -Query "hi"
#
#   # 调试转储：把每次 API 调用的 request/response 报文（已脱敏、不记 auth 头）落成
#   # run 目录的 logs\api-dump.jsonl，用于查「端点到底收了什么、回了什么」
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Query "hi" -DumpApi
#
#   # 协议级测试：把文件里的行**原样**喂 agent stdin（如先一条 user、再 {"type":"compact"}），
#   # 断言事件流里出现 system/context_compacted
#   powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -RawStdin raw.jsonl -ExpectEvent context_compacted
#
# 退出码：0 = 全过；1 = 断言失败 / 进程非正常退出；2 = 环境或用法错误（凭据缺 / exe 缺 / 超时）。
#
# 重要：agent.exe 的数据根 = 其 exe 所在目录；本脚本把 LUNAC_LOG_DIR 指到 run 目录，
# 于是 logs / tool-outputs / images / review-last 全部派生到 run 目录内（见 core-agent/src/log.rs
# 与 tools.rs 的 output_dir 派生），**不碰用户数据**。

[CmdletBinding()]
param(
    # 待测 agent.exe；默认 = release 构建（core-agent\target\release\agent.exe）。
    # 显式给则以此为准（不再看 -Dev）。
    [string]$AgentExe,
    # 改用 dev 构建（core-agent\target\debug\agent.exe）——**显式例外**；不加就是 release。
    # 用 -Dev 而非 -Debug：[CmdletBinding()] 的公共参数已占 -Debug，同名会报「参数名被多次定义」。
    [switch]$Dev,

    # 一条或多条 query（与 -QueryFile 二选一；都不给 = 一条内置冒烟 query）
    [string[]]$Query,
    # query 文件：每行一条，忽略空行与 # 开头
    [string]$QueryFile,

    # 协议级测试用：把文件里的行**原样**写进 agent stdin（不做 user-JSON 组装）。用于喂原始
    # JSON 行（如 {"type":"compact"}）验证 stdin 协议处理；与 -Print 互斥。见 docs/testing.md §测试 10。
    [string]$RawStdin,

    # 落盘根（默认 <仓库>\temp\agent-e2e；每次运行建独立子目录）
    [string]$OutDir,

    # 工作目录覆盖：默认每次运行建独立 workspace 当 agent 的 WorkingDirectory 与 --add-dir；
    # 给定则改用它（须**已存在**的目录）。用于构造多层 AGENTS.md 等夹具场景（见 §测试 10）。
    [string]$WorkspaceDir,

    # 运行参数
    [ValidateSet("plan", "acceptEdits", "full")]
    [string]$PermissionMode = "plan",
    [int]$TimeoutSec = 180,

    # 凭据 / 端点覆盖（不传则自动解析）
    [string]$Token,
    [string]$BaseUrl,
    [string]$Model,
    # lunac 数据根（读 config\ai.json 用；默认随目标布局：release -> app\src-tauri\target\release，
    # -Dev -> app\src-tauri\target\debug）
    [string]$ConfigRoot,
    # 读凭据的 .env（默认 app\src-tauri\.env）——**dev 侧凭据来源**（仓库源树文件）；
    # 安装 / release 版凭据在 config\ai.json（由 -ConfigRoot 决定读哪个布局）。
    [string]$EnvFile,

    # MCP 桥：给了就走 --mcp-server stdio:<该 exe>（默认不接，记忆 / 复盘需要它）。
    # 传宿主可执行文件：release = app\src-tauri\target\release\lunac.exe（默认布局），
    # dev = app\src-tauri\target\debug\lunac.exe，安装版 = 安装根目录（如 $INSTDIR\lunac.exe）。
    [string]$McpExe,
    # 远端 MCP 服务器配置文件（`{ "servers": [ {name,url,headers?,auth?}, … ] }`）——
    # 给了就注入 LUNAC_MCP_FILE，让 agent 在启动时连这些远端（Streamable HTTP）服务器。
    # 用于验证**HTTP 反向通道**（远端服务器在 tools/call 的 SSE 回包里反向发 sampling）。
    [string]$McpConfigFile,
    # 是否加 --permission-prompt-tool stdio（写类工具发审批卡；本脚本无回包器，默认不加）
    [switch]$StdioApproval,
    # 常驻路径的**审批回包器**（2026-10-11）：开着时改用可交互 stdin（System.Diagnostics.Process），
    # 读到 control_request 就地回一条 control_response（模拟用户点「允许」）。用于验证
    # MCP sampling 的授权卡等「同意后成功」的场景。默认关（关时走原有文件重定向路径，行为不变）。
    [switch]$AutoApprove,
    # 配合 -AutoApprove：对 `mcp_sampling` 授权卡回**拒绝**（其余仍放行）—— 验证「未同意 ⇒ 拒」。
    [switch]$DenySampling,
    # 给 agent 加 `--dangerously-skip-permissions`（验证它**不能**跳过 sampling 的高底线授权）。
    [switch]$DangerouslySkipPermissions,

    # 断言
    [string[]]$ExpectContains,
    [string[]]$ExpectTool,
    [switch]$ExpectNoError,
    [switch]$ExpectSubagentUsage,
    # 协议级测试用：断言事件流里出现某条 system 事件子类型（如 context_compacted）。
    [string[]]$ExpectEvent,
    # 载荷字段取证：断言某条 system 事件里含指定子串，形如 "<subtype>:<needle>"
    #（如 "user_file:report.txt" / "push_notification:构建完成"）。
    [string[]]$ExpectEventContains,
    # 断言 agent 的 stderr 里含指定子串（MCP sampling 的拒绝原因会经夹具的 stderr 落到这里）。
    [string[]]$ExpectStderr,
    # 断言常驻 + -AutoApprove 路径上收到的 `mcp_sampling` 授权卡**张数**（用于验证
    # 「本会话记住」：同一会话内连发两次 sampling，只应弹 1 张卡）。-1 = 不断言。
    [int]$ExpectSamplingCards = -1,

    # 调试转储：给子进程注入 LUNAC_DUMP_API=1，把每次 API 调用的 request/response 报文
    # （已脱敏、不记 auth 头）写到 run 目录的 logs\api-dump.jsonl（见 docs/testing.md §测试 10）
    [switch]$DumpApi,

    # 单发模式：走 agent.exe 的 --print --output-format json（一次一条 query，取一个 result JSON）。
    # 常驻 stdio 路径（默认）能给完整事件流 / 工具序列；单发路径更简单，只给最终文本 + usage + 成败。
    # 注：单发走 json 输出，不产生工具序列事件，故 -ExpectTool 需改用 stdio 路径。
    [switch]$Print,

    # 只跑不落「结构化摘要」之外的额外打印
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"

# ── 路径推导（禁止硬编码本机绝对路径）─────────────────────────────
$repoRoot = Split-Path -Parent $PSScriptRoot
# 默认瞄准 release（交付目标 = 用户实际安装 / 运行的版本）；-Dev 是显式例外。
$profileDir = if ($Dev) { "debug" } else { "release" }
if (-not $AgentExe)    { $AgentExe    = Join-Path $repoRoot ("core-agent\target\{0}\agent.exe" -f $profileDir) }
if (-not $OutDir)      { $OutDir      = Join-Path $repoRoot "temp\agent-e2e" }
if (-not $ConfigRoot)  { $ConfigRoot  = Join-Path $repoRoot ("app\src-tauri\target\{0}" -f $profileDir) }
if (-not $EnvFile)     { $EnvFile     = Join-Path $repoRoot "app\src-tauri\.env" }

function Fail-Setup([string]$msg) {
    Write-Host ("[agent-e2e] ERROR: {0}" -f $msg) -ForegroundColor Red
    exit 2
}

# ── 小工具 ───────────────────────────────────────────────────────
function Read-Utf8Lines([string]$path) {
    return [System.IO.File]::ReadAllLines($path, [System.Text.Encoding]::UTF8)
}
function Read-Utf8Text([string]$path) {
    # 用 FileShare.ReadWrite 打开：Start-Process 的 stdout/stderr 重定向写句柄可能被继承给
    # agent 的 MCP 子进程、在它退出前仍是打开的；ReadAllText 默认的 FileShare.Read 不许
    # 该写句柄共存 ⇒ 偶发「文件被另一进程占用」。只读语义不变，只是允许共享打开。
    $fs = [System.IO.File]::Open($path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        $sr = New-Object System.IO.StreamReader($fs, [System.Text.Encoding]::UTF8)
        try { return $sr.ReadToEnd() } finally { $sr.Dispose() }
    }
    finally { $fs.Dispose() }
}

# 参数转单个命令行字符串（PS 5.1 无 ArgumentList，需自行加引号）
# 两条路径（常驻 stdio / 单发 -Print）都用到，故定义在派发之前。
function Quote-Arg([string]$a) {
    if ($a -match '[\s"]') { return '"' + ($a -replace '"', '\"') + '"' }
    return $a
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

# 取非空字符串：优先候选顺序里第一个「有值」的
function First-NonEmpty([object[]]$values) {
    foreach ($v in $values) {
        if ($null -ne $v) {
            $s = [string]$v
            if ($s.Trim() -ne "") { return $s.Trim() }
        }
    }
    return $null
}

# 端点构造：与宿主 commands.rs 的 agent_endpoint 同规则 ——
# 有 AI_AGENT_URL 就用它；否则剥末尾 /v1 再拼 /anthropic。
# （agent 侧 Cfg::from_env 会再拼 /v1/messages。）
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

# 脱敏：把真 token 与 sk- 形态一律打码（进屏幕 / 落盘前统一过一遍）
function Protect-Text([string]$text, [string]$secret) {
    if ($null -eq $text) { return "" }
    $out = $text
    if ($secret -and $secret.Trim() -ne "") {
        $out = $out.Replace($secret, "***")
    }
    $out = [regex]::Replace($out, 'sk-[A-Za-z0-9_\-]{6,}', 'sk-***')
    return $out
}

# ── 解析凭据 ─────────────────────────────────────────────────────
$envMap = Get-DotEnvMap $EnvFile

$aiJson = $null
$aiPath = Join-Path $ConfigRoot "config\ai.json"
if (Test-Path -LiteralPath $aiPath) {
    try { $aiJson = (Read-Utf8Text $aiPath) | ConvertFrom-Json } catch { $aiJson = $null }
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

if (-not (Test-Path -LiteralPath $AgentExe)) {
    $msgNl = [string][char]10
    if ($Dev) {
        Fail-Setup ("dev 构建 agent.exe 不存在: {0}{1}先构建：cargo build（cwd=core-agent）。{1}-Dev 是显式指回 dev；不加则默认用 release。" -f $AgentExe, $msgNl)
    } else {
        Fail-Setup ("release 构建 agent.exe 不存在: {0}{1}先构建：cargo build --release（cwd=core-agent）。{1}默认瞄准 release 是硬纪律、不会静默回落到 debug；要测 dev 请显式加 -Dev。" -f $AgentExe, $msgNl)
    }
}
if (-not $token) {
    Fail-Setup ("没有可用的 API key：请配好 {0} 的 AI_API_KEY，或设 env / 传 -Token" -f $EnvFile)
}

# ── 组装 query 列表 ──────────────────────────────────────────────
$queries = @()
if ($QueryFile) {
    if (-not (Test-Path -LiteralPath $QueryFile)) { Fail-Setup ("query 文件读不到: {0}" -f $QueryFile) }
    foreach ($ln in (Read-Utf8Lines $QueryFile)) {
        $t = $ln.Trim()
        if ($t -eq "" -or $t.StartsWith("#")) { continue }
        $queries += $t
    }
}
if ($Query) { $queries += $Query }
if ($queries.Count -eq 0 -and -not $RawStdin) { $queries = @("Reply with the single word: PONG") }

# -RawStdin：协议级测试用（把文件行原样喂 stdin）。与 -Print（恰好一轮提问）互斥。
if ($RawStdin) {
    if ($Print) { Fail-Setup "-RawStdin 与 -Print 互斥：协议级原样 stdin 请走常驻 stdio 路径（去掉 -Print）" }
    if (-not (Test-Path -LiteralPath $RawStdin)) { Fail-Setup ("-RawStdin 文件读不到: {0}" -f $RawStdin) }
}

# ── 建 run 目录（每次独立，含全部落盘）───────────────────────────
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $OutDir $stamp
$logDir = Join-Path $runDir "logs"
$workDir = Join-Path $runDir "workspace"
# agent 侧开启转储时，api-dump.jsonl 落在 LUNAC_LOG_DIR（= 本 run 的 logs 目录）下
$apiDumpPath = Join-Path $logDir "api-dump.jsonl"
[void](New-Item -ItemType Directory -Force -Path $logDir)
# 工作目录：默认新建独立 workspace；-WorkspaceDir 给定则改用它（已存在），用于夹具场景
if ($WorkspaceDir) {
    if (-not (Test-Path -LiteralPath $WorkspaceDir -PathType Container)) {
        Fail-Setup ("-WorkspaceDir 不存在或不是目录: {0}" -f $WorkspaceDir)
    }
    $workDir = (Resolve-Path -LiteralPath $WorkspaceDir).Path
} else {
    [void](New-Item -ItemType Directory -Force -Path $workDir)
}

$nl = [string][char]10

# ── 单发路径（-Print）：一次一条 query，走 --print --output-format json ──────────
# 适用场景：只要「最终文本 / usage / 成败」、不关心常驻事件流时，比 stdio 路径简单直接。
# 做法：每条 query 拉起一个 agent.exe（--print --output-format json），把 query 原文写进 stdin、
# 关 stdin、等退出；stdout 恰好一行 result JSON。各条结果拼进同一个 stream.jsonl。
if ($Print) {
    if ($ExpectTool) {
        Fail-Setup "-Print 走 --output-format json，不产生工具序列事件；要断工具序列请改用 stdio 路径（去掉 -Print）"
    }

    $envKeys = @("LUNAC_AGENT_BASE_URL", "LUNAC_AGENT_TOKEN", "LUNAC_AGENT_MODEL",
                 "LUNAC_LOG_DIR", "LUNAC_LOG_LEVEL", "LUNAC_THINKING", "LUNAC_DUMP_API", "LUNAC_MCP_FILE")
    $envBackup = @{}
    foreach ($k in $envKeys) { $envBackup[$k] = [Environment]::GetEnvironmentVariable($k, "Process") }
    $env:LUNAC_AGENT_BASE_URL = $endpoint
    $env:LUNAC_AGENT_TOKEN = $token
    $env:LUNAC_AGENT_MODEL = $model
    $env:LUNAC_LOG_DIR = $logDir
    $env:LUNAC_LOG_LEVEL = "info"
    $env:LUNAC_THINKING = "on"
    if ($McpConfigFile) { $env:LUNAC_MCP_FILE = (Resolve-Path -LiteralPath $McpConfigFile).Path } else { $env:LUNAC_MCP_FILE = "" }
    if ($DumpApi) { $env:LUNAC_DUMP_API = "1" } else { $env:LUNAC_DUMP_API = "" }

    $perQuery = New-Object System.Collections.ArrayList
    $exitCode = 0
    $timedOut = $false
    $stdoutFile = Join-Path $runDir "stream.jsonl"
    $stderrFile = Join-Path $runDir "stderr.log"
    $allOut = New-Object System.Text.StringBuilder
    $allErr = New-Object System.Text.StringBuilder
    try {
        $idx = 0
        foreach ($q in $queries) {
            $idx++
            $stdinFile = Join-Path $runDir ("stdin-{0}.txt" -f $idx)
            $outFile = Join-Path $runDir ("stdout-{0}.jsonl" -f $idx)
            $errFile = Join-Path $runDir ("stderr-{0}.log" -f $idx)
            [System.IO.File]::WriteAllText($stdinFile, $q, (New-Object System.Text.UTF8Encoding($false)))
            $pArgs = @("--print", "--output-format", "json",
                       "--permission-mode", $PermissionMode,
                       "--add-dir", $workDir)
            if ($DangerouslySkipPermissions) { $pArgs += "--dangerously-skip-permissions" }
            if ($McpExe) { $pArgs += @("--mcp-server", ("stdio:" + $McpExe)) }
            $pArgs += "."
            $pArgStr = (($pArgs | ForEach-Object { Quote-Arg $_ }) -join " ")
            $spArgs = @{
                FilePath               = $AgentExe
                ArgumentList           = $pArgStr
                WorkingDirectory       = $workDir
                RedirectStandardInput  = $stdinFile
                RedirectStandardOutput = $outFile
                RedirectStandardError  = $errFile
                PassThru               = $true
                NoNewWindow            = $true
            }
            $proc = Start-Process @spArgs
            if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
                $timedOut = $true
                try { $proc.Kill() } catch { }
                [void]$proc.WaitForExit(5000)
            }
            else { [void]$proc.WaitForExit() }
            $rc = $proc.ExitCode
            if ($null -eq $rc) { $rc = 0 }
            if ($rc -ne 0) { $exitCode = 1 }
            $outTxt = ""
            $errTxt = ""
            if (Test-Path -LiteralPath $outFile) { $outTxt = Read-Utf8Text $outFile }
            if (Test-Path -LiteralPath $errFile) { $errTxt = Read-Utf8Text $errFile }
            [void]$allOut.Append($outTxt)
            [void]$allErr.Append($errTxt)
            # 单发 stdout 恰好一行 result JSON（其余事件被 agent 侧按 json 模式抑制）
            $res = $null
            foreach ($ln in ($outTxt -split "\r?\n")) {
                $t = $ln.Trim()
                if ($t.StartsWith("{")) { try { $res = $t | ConvertFrom-Json } catch { } }
            }
            if ($res) {
                [void]$perQuery.Add([ordered]@{
                    final_text   = [string]$res.result
                    assistant    = ""
                    is_error     = [bool]$res.is_error
                    subtype      = [string]$res.subtype
                    num_turns    = $res.num_turns
                    duration_ms  = $res.duration_ms
                    tools        = @()
                    usage        = $res.usage
                    subagent_request_count = 0
                    exit_code    = $rc
                })
            }
            else {
                [void]$perQuery.Add([ordered]@{
                    final_text   = ""; assistant = ""; is_error = $true; subtype = "no_result"
                    num_turns    = 0; duration_ms = 0; tools = @(); usage = $null
                    subagent_request_count = 0; exit_code = $rc
                })
            }
        }
    }
    finally {
        foreach ($k in $envKeys) {
            if ($null -eq $envBackup[$k]) { Remove-Item -Path ("env:" + $k) -ErrorAction SilentlyContinue }
            else { Set-Item -Path ("env:" + $k) -Value $envBackup[$k] }
        }
    }
    [System.IO.File]::WriteAllText($stdoutFile, $allOut.ToString(), (New-Object System.Text.UTF8Encoding($false)))
    [System.IO.File]::WriteAllText($stderrFile, $allErr.ToString(), (New-Object System.Text.UTF8Encoding($false)))

    # ── 断言（单发只支持 -ExpectContains / -ExpectNoError）──────────
    $failures = New-Object System.Collections.ArrayList
    $allFinal = [string]::Join($nl, @($perQuery | ForEach-Object { $_.final_text }))
    if ($timedOut) { [void]$failures.Add(("超时（{0}s）未退出，已强杀" -f $TimeoutSec)) }
    if ($perQuery.Count -lt $queries.Count) {
        [void]$failures.Add(("期望 {0} 个 result，只收到 {1} 个" -f $queries.Count, $perQuery.Count))
    }
    if ($ExpectContains) {
        foreach ($needle in $ExpectContains) {
            if ($allFinal.IndexOf($needle) -lt 0) { [void]$failures.Add(("最终输出不含 '{0}'" -f $needle)) }
        }
    }
    if ($ExpectNoError) {
        foreach ($it in $perQuery) { if ($it.is_error) { [void]$failures.Add(("有 result 报错: {0}" -f $it.final_text)) } }
    }
    if ($ExpectStderr) {
        $errAll = $allErr.ToString()
        foreach ($needle in $ExpectStderr) {
            if ($errAll.IndexOf($needle) -lt 0) { [void]$failures.Add(("stderr 不含 '{0}'" -f $needle)) }
        }
    }

    $apiDumpLines = 0
    if (Test-Path -LiteralPath $apiDumpPath) { $apiDumpLines = @(Read-Utf8Lines $apiDumpPath).Count }
    $summary = [ordered]@{
        mode            = "print"
        agent_exe       = $AgentExe
        endpoint        = Protect-Text $endpoint $token
        model           = $model
        permission_mode = $PermissionMode
        output_format   = "json"
        queries         = $queries
        skip_permissions = [bool]$DangerouslySkipPermissions
        exit_code       = $exitCode
        timed_out       = $timedOut
        dump_api        = [bool]$DumpApi
        api_dump_path   = $apiDumpPath
        api_dump_lines  = $apiDumpLines
        tool_pool       = @()
        results         = @($perQuery)
        failures        = @($failures)
    }
    $summaryJson = Protect-Text ($summary | ConvertTo-Json -Depth 12) $token
    $summaryPath = Join-Path $runDir "summary.json"
    [System.IO.File]::WriteAllText($summaryPath, $summaryJson, (New-Object System.Text.UTF8Encoding($false)))

    if (-not $Quiet) {
        Write-Host "[agent-e2e] mode    : print (single-shot, --print --output-format json)" -ForegroundColor DarkGray
        Write-Host "[agent-e2e] run dir : $runDir" -ForegroundColor DarkGray
        Write-Host ("[agent-e2e] model   : {0}  endpoint: {1}" -f $model, (Protect-Text $endpoint $token)) -ForegroundColor DarkGray
        $i = 0
        foreach ($it in $perQuery) {
            $i++
            Write-Host ("--- query #{0}  turns={1} dur={2}ms  err={3}  exit={4}" -f $i, $it.num_turns, $it.duration_ms, $it.is_error, $it.exit_code)
            $shown = Protect-Text $it.final_text $token
            if ($shown.Length -gt 600) { $shown = $shown.Substring(0, 600) + " ...[truncated]" }
            Write-Host ("    final: {0}" -f $shown)
            if ($it.usage) {
                Write-Host ("    usage: in={0} out={1} cache_read={2} cache_create={3}" -f $it.usage.input_tokens, $it.usage.output_tokens, $it.usage.cache_read_input_tokens, $it.usage.cache_creation_input_tokens)
            }
        }
        Write-Host ""
    }
    Write-Host ("[agent-e2e] stream -> {0}" -f $stdoutFile) -ForegroundColor DarkGray
    Write-Host ("[agent-e2e] summary -> {0}" -f $summaryPath) -ForegroundColor DarkGray
    if ($DumpApi) {
        Write-Host ("[agent-e2e] api-dump -> {0}  ({1} 行，已脱敏；不记 auth 头)" -f $apiDumpPath, $apiDumpLines) -ForegroundColor DarkGray
    }

    if ($timedOut) {
        Write-Host "[agent-e2e] FAIL: 超时未退出（agent.exe 可能卡在审批等待或网络）" -ForegroundColor Red
        exit 1
    }
    if ($failures.Count -gt 0) {
        Write-Host "[agent-e2e] FAIL:" -ForegroundColor Red
        foreach ($f in $failures) { Write-Host ("  - {0}" -f (Protect-Text $f $token)) -ForegroundColor Red }
        exit 1
    }
    if ($exitCode -ne 0) {
        Write-Host ("[agent-e2e] FAIL: agent.exe 存在非 0 退出（stderr 见 {0}）" -f $stderrFile) -ForegroundColor Red
        exit 1
    }
    Write-Host "[agent-e2e] PASS" -ForegroundColor Green
    exit 0
}


# ── 组装参数（常驻 stdio 路径；与宿主 start_cli_process 同形，可覆盖）─────────────
# 常驻模式的判据是 --input-format stream-json（见 core-agent CliArgs::single_shot）——
# 单发 --print 是「带 --print 且**未**声明 stream-json 输入」才生效；这里明确声明了
# stream-json 输入，故恒为常驻。同时不再传 --print，避免与单发开关的语义混淆。
$argList = New-Object System.Collections.ArrayList
[void]$argList.AddRange(@(
    "--verbose",
    "--input-format", "stream-json",
    "--output-format", "stream-json",
    "--include-partial-messages"
))
if ($StdioApproval) { [void]$argList.AddRange(@("--permission-prompt-tool", "stdio")) }
if ($DangerouslySkipPermissions) { [void]$argList.Add("--dangerously-skip-permissions") }
if ($McpExe) { [void]$argList.AddRange(@("--mcp-server", ("stdio:" + $McpExe))) }
[void]$argList.AddRange(@("--permission-mode", $PermissionMode))
[void]$argList.AddRange(@("--add-dir", $workDir))
[void]$argList.Add(".")

# 参数转单个命令行字符串（PS 5.1 无 ArgumentList，需自行加引号）
$argStr = (($argList | ForEach-Object { Quote-Arg $_ }) -join " ")

# ── 组装 stdin（每行一条 user JSON）─────────────────────────────
# 非 ASCII 转成 \uXXXX，使 stdin 纯 ASCII —— 免去对进程 stdin 编码的依赖
# （ProcessStartInfo.StandardInputEncoding 在本机 .NET Framework 上不可设）。
function Get-AsciiJson([string]$json) {
    $sb = New-Object System.Text.StringBuilder
    foreach ($ch in $json.ToCharArray()) {
        if ([int]$ch -gt 127) { [void]$sb.AppendFormat("\u{0:X4}", [int]$ch) }
        else { [void]$sb.Append($ch) }
    }
    return $sb.ToString()
}
$nl = [string][char]10
$inputLineList = @()
if ($RawStdin) {
    # 协议级：把文件里的行**原样**写进 stdin（不组装、不改写），末尾确保一个换行。
    $stdinText = Read-Utf8Text $RawStdin
    if (-not $stdinText.EndsWith($nl)) { $stdinText += $nl }
    $inputLineList = @(($stdinText -split "\r?\n") | Where-Object { $_ -ne "" })
} else {
    $inputLines = @()
    foreach ($q in $queries) {
        $payload = [ordered]@{
            type               = "user"
            session_id         = ""
            message            = [ordered]@{
                role    = "user"
                content = @([ordered]@{ type = "text"; text = $q })
            }
            parent_tool_use_id = $null
        }
        $inputLines += (Get-AsciiJson ($payload | ConvertTo-Json -Depth 8 -Compress))
    }
    $stdinText = ($inputLines -join $nl) + $nl
    $inputLineList = @($inputLines)
}
$stdinFile = Join-Path $runDir "stdin.jsonl"
$stdoutFile = Join-Path $runDir "stream.jsonl"
$stderrFile = Join-Path $runDir "stderr.log"
[System.IO.File]::WriteAllText($stdinFile, $stdinText, (New-Object System.Text.UTF8Encoding($false)))

# ── 拉起进程 ─────────────────────────────────────────────────────
# 用 Start-Process + 文件重定向 stdin/stdout/stderr：stdin 的字节完全由我们写（UTF-8 无 BOM），
# 不经过 Process.StandardInput 那个会带 BOM / 受编码影响的写入器（实测它会写 BOM，
# 导致 agent 首行 JSON 解析失败 →「忽略非法 JSON 输入行」）。
# env 只能经当前进程环境继承给子进程，故临时设置、跑完还原（token 不进日志）。
$envKeys = @("LUNAC_AGENT_BASE_URL", "LUNAC_AGENT_TOKEN", "LUNAC_AGENT_MODEL",
             "LUNAC_LOG_DIR", "LUNAC_LOG_LEVEL", "LUNAC_THINKING", "LUNAC_DUMP_API", "LUNAC_MCP_FILE")
$envBackup = @{}
foreach ($k in $envKeys) { $envBackup[$k] = [Environment]::GetEnvironmentVariable($k, "Process") }

$env:LUNAC_AGENT_BASE_URL = $endpoint
$env:LUNAC_AGENT_TOKEN = $token
$env:LUNAC_AGENT_MODEL = $model
$env:LUNAC_LOG_DIR = $logDir
$env:LUNAC_LOG_LEVEL = "info"
$env:LUNAC_THINKING = "on"
# 远端 MCP 服务器配置：给了就注入；否则显式置空（避免继承宿主环境里的同名变量）
if ($McpConfigFile) { $env:LUNAC_MCP_FILE = (Resolve-Path -LiteralPath $McpConfigFile).Path } else { $env:LUNAC_MCP_FILE = "" }
# 默认关：只有 -DumpApi 才注入真值；否则显式置空，避免继承宿主环境里可能存在的同名变量
if ($DumpApi) { $env:LUNAC_DUMP_API = "1" } else { $env:LUNAC_DUMP_API = "" }

$timedOut = $false
$exitCode = -1
# 本次 run 收到的 `mcp_sampling` 授权卡张数（仅交互式 -AutoApprove 路径统计）——
# 用于验证「本会话记住」（同会话内连发两次 sampling 只应弹 1 张卡）。
$samplingCards = 0
# 交互式审批回包器只在「常驻 + -AutoApprove + 非 -RawStdin」时启用；其余走原有文件重定向路径。
$useInteractive = $AutoApprove -and (-not $RawStdin)
try {
    if (-not $useInteractive) {
        $spArgs = @{
            FilePath              = $AgentExe
            ArgumentList          = $argStr
            WorkingDirectory      = $workDir
            RedirectStandardInput = $stdinFile
            RedirectStandardOutput = $stdoutFile
            RedirectStandardError = $stderrFile
            PassThru              = $true
            NoNewWindow           = $true
        }
        $proc = Start-Process @spArgs
        if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
            $timedOut = $true
            try { $proc.Kill() } catch { }
            [void]$proc.WaitForExit(5000)
        }
        else {
            # 带超时的 WaitForExit 之后必须再调一次无参版，Process.ExitCode 才可靠
            # （Start-Process -PassThru 的对象在只调带参版时 ExitCode 可能读成空）。
            [void]$proc.WaitForExit()
        }
        $exitCode = $proc.ExitCode
        if ($null -eq $exitCode) { $exitCode = 0 }
    }
    else {
        # ── 交互式：System.Diagnostics.Process + 直写 BaseStream（UTF-8 无 BOM）──────────
        # 为什么要自己写 BaseStream：ProcessStartInfo.StandardInputEncoding 在本机
        # .NET Framework 上不可设，默认写入器会带 BOM 把 agent 首行 JSON 弄坏（同上文注释）。
        # 读到 control_request 就地回 control_response —— 这就是「模拟用户在卡片上点允许」。
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName               = $AgentExe
        $psi.Arguments              = $argStr
        $psi.WorkingDirectory       = $workDir
        $psi.UseShellExecute        = $false
        $psi.CreateNoWindow         = $true
        $psi.RedirectStandardInput  = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError  = $true
        $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
        $psi.StandardOutputEncoding = $utf8NoBom
        $psi.StandardErrorEncoding  = $utf8NoBom
        $proc = New-Object System.Diagnostics.Process
        $proc.StartInfo = $psi
        [void]$proc.Start()
        $sw = New-Object System.IO.StreamWriter($proc.StandardInput.BaseStream, $utf8NoBom)
        $sw.NewLine = $nl
        $sw.AutoFlush = $true
        # 先写一个空行：实测（Windows + PS5.1 + .NET Framework）往 stdin 管道写的第一行会被吞掉
        # （agent 侧收到一条空行 → “忽略非法 JSON 输入行”，随后真正的 user 行才被读到）。
        # 空行是无害的（agent 会跳过），垫一行可保证真正的输入不被吞。
        $sw.WriteLine("")
        foreach ($ln in $inputLineList) { $sw.WriteLine($ln) }
        $errTask = $proc.StandardError.ReadToEndAsync()
        $sb = New-Object System.Text.StringBuilder
        $deadline = (Get-Date).AddSeconds($TimeoutSec)
        $results = 0
        $t = $proc.StandardOutput.ReadLineAsync()
        while ($true) {
            if (-not $t.IsCompleted) { [void]$t.Wait(250) }
            if (-not $t.IsCompleted) {
                if ($proc.HasExited) { break }
                if ((Get-Date) -ge $deadline) { $timedOut = $true; break }
                continue
            }
            $line = $null
            try { $line = $t.Result } catch { break }
            if ($null -eq $line) { break }
            [void]$sb.AppendLine($line)
            $trimmed = $line.Trim()
            if ($trimmed.StartsWith("{")) {
                $evt = $null
                try { $evt = $trimmed | ConvertFrom-Json } catch { $evt = $null }
                if ($evt -and [string]$evt.type -eq "control_request" -and $evt.request_id) {
                    $sub = [string]$evt.request.subtype
                    if ($sub -eq "mcp_sampling") {
                        $samplingCards++
                        Write-Host ("[agent-e2e] sampling card #{0}: server={1} viaHttp={2}" -f $samplingCards, $evt.request.server, $evt.request.viaHttp) -ForegroundColor DarkGray
                    }
                    $allow = $true
                    if ($sub -eq "mcp_sampling" -and $DenySampling) { $allow = $false }
                    $inner = '{"behavior":"allow","updatedInput":{}}'
                    if (-not $allow) {
                        $inner = '{"behavior":"deny","message":"Denied by e2e fixture","interrupt":false}'
                    }
                    $resp = '{"type":"control_response","response":{"subtype":"success","request_id":"' +
                            [string]$evt.request_id + '","response":' + $inner + '}}'
                    $sw.WriteLine($resp)
                }
                elseif ($evt -and [string]$evt.type -eq "result") {
                    $results++
                    if ($results -ge $queries.Count) { break }
                }
            }
            $t = $proc.StandardOutput.ReadLineAsync()
        }
        # 关 stdin → 常驻 agent 收尾退出
        try { $sw.Flush(); $sw.Dispose(); $proc.StandardInput.Close() } catch { }
        if (-not $proc.WaitForExit(10000)) {
            $timedOut = $true
            try { $proc.Kill() } catch { }
            [void]$proc.WaitForExit(5000)
        }
        $exitCode = $proc.ExitCode
        if ($null -eq $exitCode) { $exitCode = 0 }
        $errText = ""
        try { $errText = $errTask.Result } catch { }
        [System.IO.File]::WriteAllText($stdoutFile, $sb.ToString(), $utf8NoBom)
        [System.IO.File]::WriteAllText($stderrFile, $errText, $utf8NoBom)
    }
}
finally {
    foreach ($k in $envKeys) {
        if ($null -eq $envBackup[$k]) { Remove-Item -Path ("env:" + $k) -ErrorAction SilentlyContinue }
        else { Set-Item -Path ("env:" + $k) -Value $envBackup[$k] }
    }
}

$stdoutText = ""
$stderrText = ""
if (Test-Path -LiteralPath $stdoutFile) { $stdoutText = Read-Utf8Text $stdoutFile }
if (Test-Path -LiteralPath $stderrFile) { $stderrText = Read-Utf8Text $stderrFile }

# ── 解析事件流 ───────────────────────────────────────────────────
$events = New-Object System.Collections.ArrayList
foreach ($ln in ($stdoutText -split "\r?\n")) {
    $t = $ln.Trim()
    if ($t -eq "" -or -not $t.StartsWith("{")) { continue }
    try { [void]$events.Add(($t | ConvertFrom-Json)) } catch { }
}

$toolPool = @()
$perQuery = New-Object System.Collections.ArrayList
$curTools = New-Object System.Collections.ArrayList
$curAsstText = New-Object System.Collections.ArrayList
# 全量 system 事件子类型（-ExpectEvent 断言用；协议级测试的证据来源）
$sysSubtypes = New-Object System.Collections.ArrayList

foreach ($e in $events) {
    $etype = [string]$e.type
    if ($etype -eq "system" -and $e.subtype) { [void]$sysSubtypes.Add([string]$e.subtype) }
    if ($etype -eq "system" -and [string]$e.subtype -eq "init") {
        foreach ($tn in @($e.tools)) { $toolPool += [string]$tn }
        continue
    }
    if ($etype -eq "assistant") {
        $content = $e.message.content
        foreach ($b in @($content)) {
            if ([string]$b.type -eq "tool_use") { [void]$curTools.Add([string]$b.name) }
            elseif ([string]$b.type -eq "text") { [void]$curAsstText.Add([string]$b.text) }
        }
        continue
    }
    if ($etype -eq "result") {
        $reqs = @()
        $sub = $null
        if ($e.usage) {
            if ($e.usage.requests) { $reqs = @($e.usage.requests) }
            if ($e.usage.subagent) { $sub = $e.usage.subagent }
        }
        $subReq = 0
        if ($sub -and $sub.requests) { $subReq = @($sub.requests).Count }
        $item = [ordered]@{
            final_text   = [string]$e.result
            assistant    = [string]::Join("", @($curAsstText))
            is_error     = [bool]$e.is_error
            subtype      = [string]$e.subtype
            num_turns    = $e.num_turns
            duration_ms  = $e.duration_ms
            tools        = @($curTools)
            usage        = $e.usage
            subagent_request_count = $subReq
        }
        [void]$perQuery.Add($item)
        $curTools = New-Object System.Collections.ArrayList
        $curAsstText = New-Object System.Collections.ArrayList
        continue
    }
}

# ── 断言 ─────────────────────────────────────────────────────────
$failures = New-Object System.Collections.ArrayList
$allFinal = [string]::Join($nl, @($perQuery | ForEach-Object { $_.final_text }))
$allTools = @($perQuery | ForEach-Object { $_.tools } | ForEach-Object { $_ })

if ($timedOut) { [void]$failures.Add(("超时（{0}s）未退出，已强杀" -f $TimeoutSec)) }
if (-not $RawStdin -and $perQuery.Count -lt $queries.Count) {
    [void]$failures.Add(("期望 {0} 个 result，只收到 {1} 个" -f $queries.Count, $perQuery.Count))
}
if ($ExpectContains) {
    foreach ($needle in $ExpectContains) {
        if ($allFinal.IndexOf($needle) -lt 0) {
            [void]$failures.Add(("最终输出不含 '{0}'" -f $needle))
        }
    }
}
if ($ExpectTool) {
    foreach ($needle in $ExpectTool) {
        if ($allTools -notcontains $needle) {
            [void]$failures.Add(("工具序列里没有 '{0}'（实际: {1}）" -f $needle, ($allTools -join ",")))
        }
    }
}
if ($ExpectNoError) {
    foreach ($it in $perQuery) {
        if ($it.is_error) { [void]$failures.Add(("有 result 报错: {0}" -f $it.final_text)) }
    }
}
if ($ExpectSubagentUsage) {
    $any = $false
    foreach ($it in $perQuery) { if ($it.subagent_request_count -gt 0) { $any = $true } }
    if (-not $any) { [void]$failures.Add("没有任何 result 带子代理 / 复盘用量（usage.subagent.requests 全 0；复盘需 -McpExe 接桥）") }
}
# 协议级断言：事件流里出现指定的 system 事件子类型（如 context_compacted）
if ($ExpectEvent) {
    foreach ($needle in $ExpectEvent) {
        if ($sysSubtypes -notcontains $needle) {
            [void]$failures.Add(("事件流里没有 system/{0}（实际: {1}）" -f $needle, ($sysSubtypes -join ",")))
        }
    }
}
# 载荷字段取证：某条 system 事件（JSON）里含指定子串。规格 "<subtype>:<needle>"。
if ($ExpectEventContains) {
    foreach ($spec in $ExpectEventContains) {
        $i = $spec.IndexOf(":")
        if ($i -lt 1) {
            [void]$failures.Add((" -ExpectEventContains 格式应为 <subtype>:<needle>，收到 '{0}'" -f $spec))
            continue
        }
        $sub = $spec.Substring(0, $i)
        $needle = $spec.Substring($i + 1)
        $hit = $false
        foreach ($e in $events) {
            if ([string]$e.type -eq "system" -and [string]$e.subtype -eq $sub) {
                if (($e | ConvertTo-Json -Compress -Depth 10).IndexOf($needle) -ge 0) { $hit = $true; break }
            }
        }
        if (-not $hit) {
            [void]$failures.Add(("没有 system/{0} 事件含 '{1}'" -f $sub, $needle))
        }
    }
}
# stderr 断言：MCP sampling 的拒绝原因会经夹具的 stderr（inherit 到 agent stderr）落到这里
if ($ExpectStderr) {
    foreach ($needle in $ExpectStderr) {
        if ($stderrText.IndexOf($needle) -lt 0) {
            [void]$failures.Add(("stderr 不含 '{0}'" -f $needle))
        }
    }
}
# 「本会话记住」断言：常驻 + -AutoApprove 路径上收到的 mcp_sampling 授权卡张数
if ($ExpectSamplingCards -ge 0) {
    if ($samplingCards -ne $ExpectSamplingCards) {
        [void]$failures.Add(("sampling 授权卡张数不符：期望 {0}，实际 {1}（仅 -AutoApprove 常驻路径统计）" -f $ExpectSamplingCards, $samplingCards))
    }
}

# ── 结构化摘要落盘（脱敏）───────────────────────────────────────
# 转储文件是否真的产生、有几行（-DumpApi 时才可能有）
$apiDumpLines = 0
if (Test-Path -LiteralPath $apiDumpPath) {
    $apiDumpLines = @(Read-Utf8Lines $apiDumpPath).Count
}
$summary = [ordered]@{
    agent_exe      = $AgentExe
    endpoint       = Protect-Text $endpoint $token
    model          = $model
    permission_mode = $PermissionMode
    queries        = $queries
    raw_stdin      = [bool]$RawStdin
    auto_approve   = [bool]$AutoApprove
    deny_sampling  = [bool]$DenySampling
    sampling_cards = $samplingCards
    skip_permissions = [bool]$DangerouslySkipPermissions
    exit_code      = $exitCode
    timed_out      = $timedOut
    dump_api       = [bool]$DumpApi
    api_dump_path  = $apiDumpPath
    api_dump_lines = $apiDumpLines
    tool_pool      = @($toolPool)
    system_events  = @($sysSubtypes)
    results        = @($perQuery)
    failures       = @($failures)
}
$summaryJson = ($summary | ConvertTo-Json -Depth 12)
$summaryJson = Protect-Text $summaryJson $token
$summaryPath = Join-Path $runDir "summary.json"
[System.IO.File]::WriteAllText($summaryPath, $summaryJson, (New-Object System.Text.UTF8Encoding($false)))

# ── 打印（脱敏）─────────────────────────────────────────────────
if (-not $Quiet) {
    Write-Host "[agent-e2e] run dir : $runDir" -ForegroundColor DarkGray
    Write-Host ("[agent-e2e] agent   : {0}" -f $AgentExe) -ForegroundColor DarkGray
    Write-Host ("[agent-e2e] model   : {0}  endpoint: {1}" -f $model, (Protect-Text $endpoint $token)) -ForegroundColor DarkGray
    Write-Host ("[agent-e2e] queries : {0}  工具池 {1} 个" -f $queries.Count, $toolPool.Count) -ForegroundColor DarkGray
    Write-Host ""
    $i = 0
    foreach ($it in $perQuery) {
        $i++
        Write-Host ("--- query #{0}  turns={1} dur={2}ms  err={3}  tools=[{4}]  subagent_reqs={5}" -f $i, $it.num_turns, $it.duration_ms, $it.is_error, ($it.tools -join ","), $it.subagent_request_count)
        $shown = Protect-Text $it.final_text $token
        if ($shown.Length -gt 600) { $shown = $shown.Substring(0, 600) + " ...[truncated]" }
        Write-Host ("    final: {0}" -f $shown)
        if ($it.usage) {
            $inTok = $it.usage.input_tokens; $outTok = $it.usage.output_tokens
            $cr = $it.usage.cache_read_input_tokens; $cc = $it.usage.cache_creation_input_tokens
            Write-Host ("    usage: in={0} out={1} cache_read={2} cache_create={3}" -f $inTok, $outTok, $cr, $cc)
            $rq = @($it.usage.requests)
            if ($rq.Count -gt 0) {
                $j = 0
                foreach ($r in $rq) {
                    $j++
                    Write-Host ("      req#{0}: in={1} read={2} create={3} out={4}" -f $j, $r.in, $r.read, $r.create, $r.out)
                }
            }
        }
    }
    Write-Host ""
}
Write-Host ("[agent-e2e] stream -> {0}" -f $stdoutFile) -ForegroundColor DarkGray
Write-Host ("[agent-e2e] summary -> {0}" -f $summaryPath) -ForegroundColor DarkGray
if ($DumpApi) {
    Write-Host ("[agent-e2e] api-dump -> {0}  ({1} 行，已脱敏；不记 auth 头)" -f $apiDumpPath, $apiDumpLines) -ForegroundColor DarkGray
}

if ($timedOut) {
    Write-Host ("[agent-e2e] FAIL: 超时未退出（agent.exe 可能卡在审批等待或网络）") -ForegroundColor Red
    exit 1
}
if ($failures.Count -gt 0) {
    Write-Host "[agent-e2e] FAIL:" -ForegroundColor Red
    foreach ($f in $failures) { Write-Host ("  - {0}" -f (Protect-Text $f $token)) -ForegroundColor Red }
    exit 1
}
if ($exitCode -ne 0) {
    Write-Host ("[agent-e2e] FAIL: agent.exe 退出码 {0}（stderr 见 {1}）" -f $exitCode, $stderrFile) -ForegroundColor Red
    exit 1
}
Write-Host "[agent-e2e] PASS" -ForegroundColor Green
exit 0

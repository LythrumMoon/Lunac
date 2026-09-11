# scripts/commit.ps1
# 本地一键提交：暂存全部变更 → 安全检查 → 提交（可选推送）。
#
# 用法:
#   npm run commit                          （仓库根，无参数：自动生成提交信息）
#   powershell -ExecutionPolicy Bypass -File scripts\commit.ps1 -DryRun
#   powershell -ExecutionPolicy Bypass -File scripts\commit.ps1 -Message "fix: 修复搜索抖动" -Push
#
#   注意：`npm run commit -- -DryRun` 在本机 PowerShell 下不会把参数转发给脚本
#   （npm.ps1 会吃掉 `--` 之后的开关），**带参数时请直接调用 .ps1**。
#
# 设计要点:
#   - 提交信息经 UTF-8（无 BOM）临时文件交给 `git commit -F`：
#     PowerShell 直接传中文参数会被转成 GBK，git 端就是乱码。
#   - 提交前扫描敏感文件（.env / *.pem / *.key / credentials.json）与超大文件
#     （GitHub 单文件 100MB 上限），命中即中止并回退暂存。
#   - 默认只提交不推送；-Push 才推。永不 force push。

param(
  [string]$Message = "",
  [switch]$Push,
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot

function Fail($m) {
  Write-Host "  [FAIL] $m" -ForegroundColor Red
  exit 1
}

Push-Location $Root
try {
  # ── 0. 必须位于 git 仓库 ────────────────────────────────────────
  git rev-parse --is-inside-work-tree 2>$null | Out-Null
  if ($LASTEXITCODE -ne 0) { Fail "不是 git 仓库: $Root" }

  # ── 1. 有无变更 ────────────────────────────────────────────────
  $porcelain = @(git status --porcelain)
  if ($porcelain.Count -eq 0) {
    Write-Host "工作区干净，无变更可提交。" -ForegroundColor Green
    exit 0
  }

  Write-Host ""
  Write-Host "待提交变更：" -ForegroundColor Cyan
  $porcelain | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }
  Write-Host ""

  # ── 2. 暂存全部变更（含新增与删除）─────────────────────────────
  git add -A
  if ($LASTEXITCODE -ne 0) { Fail "git add 失败" }

  $staged = @(git diff --cached --name-only)
  if ($staged.Count -eq 0) { Fail "暂存区为空（文件可能被 .gitignore 忽略）" }

  # ── 3. 安全检查：敏感文件 / 超大文件 ───────────────────────────
  $sensitive = @()
  $tooLarge  = @()
  foreach ($f in $staged) {
    $leaf = Split-Path $f -Leaf
    $isEnv = ($leaf -eq ".env") -or ($leaf -like ".env.*" -and $leaf -ne ".env.example")
    if ($isEnv -or $leaf -match '\.(pem|key|pfx|p12)$' -or $leaf -eq "credentials.json") {
      $sensitive += $f
    }
    $full = Join-Path $Root $f
    if (Test-Path -LiteralPath $full -PathType Leaf) {
      $len = (Get-Item -LiteralPath $full).Length
      if ($len -gt 50MB) { $tooLarge += ("{0}（{1} MB）" -f $f, [math]::Round($len / 1MB, 1)) }
    }
  }

  if ($sensitive.Count -gt 0) {
    git reset -q
    Write-Host "拒绝提交：检测到疑似密钥文件（暂存已回退）" -ForegroundColor Red
    $sensitive | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    Write-Host "  若确需提交，请先确认该文件不含真实凭据，再用 git 手动处理。" -ForegroundColor DarkGray
    exit 1
  }
  if ($tooLarge.Count -gt 0) {
    git reset -q
    Write-Host "拒绝提交：以下文件超过 50MB（GitHub 单文件上限 100MB）" -ForegroundColor Red
    $tooLarge | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    Write-Host "  建议加入 .gitignore 或改用 Release 附件分发。" -ForegroundColor DarkGray
    exit 1
  }

  # ── 4. 提交信息（未指定则自动生成）─────────────────────────────
  if (-not $Message) {
    $tops = $staged | ForEach-Object {
      $p = $_ -replace '\\', '/'
      if ($p.Contains('/')) { ($p -split '/')[0] } else { "根目录" }
    } | Sort-Object -Unique
    $topStr = ($tops | Select-Object -First 3) -join "、"
    if ($tops.Count -gt 3) { $topStr += " 等" }
    $Message = "chore: 更新 $topStr（$($staged.Count) 个文件）"
  }

  # ── 5. 提交 ────────────────────────────────────────────────────
  # 中文提交信息必须走 -F 文件：PowerShell 参数传递会把非 ASCII 转成 GBK。
  $msgFile = Join-Path $env:TEMP ("lunac-commit-{0}.txt" -f [guid]::NewGuid().ToString("N"))
  try {
    [System.IO.File]::WriteAllText($msgFile, $Message, [System.Text.UTF8Encoding]::new($false))

    if ($DryRun) {
      git reset -q
      Write-Host "DRY-RUN：未提交。将使用的提交信息：" -ForegroundColor Yellow
      Write-Host "  $Message" -ForegroundColor White
      exit 0
    }

    git commit -F $msgFile
    if ($LASTEXITCODE -ne 0) { Fail "git commit 失败" }
  } finally {
    Remove-Item $msgFile -Force -ErrorAction SilentlyContinue
  }

  # ── 6. 可选推送 ────────────────────────────────────────────────
  if ($Push) {
    $branch = (git rev-parse --abbrev-ref HEAD).Trim()
    git rev-parse --abbrev-ref "@{u}" 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { git push } else { git push -u origin $branch }
    if ($LASTEXITCODE -ne 0) {
      Write-Host "推送失败 —— 本地提交已完成，可稍后手动 git push。" -ForegroundColor Yellow
      exit 1
    }
  }

  # ── 7. 结果 ────────────────────────────────────────────────────
  Write-Host ""
  Write-Host "已提交：" -ForegroundColor Green
  git log -1 --format="  %h  %s"
  if (-not $Push) {
    Write-Host "  （未推送；-Push 可同时推送：powershell -File scripts\commit.ps1 -Push）" -ForegroundColor DarkGray
  }
  Write-Host ""
} finally {
  Pop-Location
}

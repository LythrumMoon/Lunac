# scripts/_env.ps1 — 公共环境准备，由其它脚本 dot-source：
#   . "$PSScriptRoot\_env.ps1"
#
# 只把「确实存在」的目录追加进 PATH：
#   · cargo（Rust 工具链）
#   · <仓库根>\mingw64\bin（本机自备的 gcc 工具链；未入库，缺失时静默跳过）
# 目的：脚本内不出现任何本机绝对路径，仓库克隆到别的机器也能直接跑。

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

$mingwBin = Join-Path (Split-Path -Parent $PSScriptRoot) "mingw64\bin"
if (Test-Path $mingwBin) { $env:PATH = "$mingwBin;$env:PATH" }

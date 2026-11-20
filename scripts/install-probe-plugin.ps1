# 把「信道探针」插件装进某个 Lunac 实例的 Modules\（§待办 3-10 的 dev 真机验收用）
#
# 为什么单列一个脚本、而不是进 build-plugins.ps1：
#   探针插件是**开发自检件**，不该出现在插件市场 / 安装包里 —— 这条脚本只往本机目录铺一份。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\install-probe-plugin.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\install-probe-plugin.ps1 -Root D:\Lunac
#
# 默认目标 = dev 实例（`app\src-tauri\target\debug`）；装到别处用 -Root 指那个 exe 根。
# 装完去「设置 → 插件 → 重新扫描」，再从搜索栏打开「信道探针」。

param(
  [string]$Root = ""
)

$ErrorActionPreference = "Stop"

# 仓库根：本文件在 scripts\ 下 ⇒ 上跳一级
$Repo = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$Src = Join-Path $Repo "scripts\probe-plugin"
if (-not (Test-Path (Join-Path $Src "lunac-plugin.json"))) {
  throw "找不到探针源：$Src"
}
if ([string]::IsNullOrWhiteSpace($Root)) {
  $Root = Join-Path $Repo "app\src-tauri\target\debug"
}
if (-not (Test-Path $Root)) {
  throw "目标实例不存在：$Root（先用 npm run tauri:dev 跑起来，或用 -Root 指到安装根）"
}

$Dest = Join-Path $Root "Modules\probe"
if (Test-Path $Dest) {
  Remove-Item -Recurse -Force $Dest
}
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
Copy-Item -Path (Join-Path $Src "*") -Destination $Dest -Recurse -Force

# 自检：清单 / 入口 / iframe 文档三件缺一不可（缺了宿主会判坏包）
foreach ($m in @("lunac-plugin.json", "index.js", "ui\index.html")) {
  if (-not (Test-Path (Join-Path $Dest $m))) {
    throw "复制后缺文件：$m（探针不完整）"
  }
}

Write-Host "探针已装：$Dest" -ForegroundColor Green
Write-Host "下一步：设置 → 插件 → 重新扫描（或重启 Lunac），再从搜索栏打开「信道探针」" -ForegroundColor DarkGray
Write-Host "判据：面板里出现 hello → ready 两行；两个常驻锚点（聊天侧栏 / 设置分组）各出现一块探针" -ForegroundColor DarkGray

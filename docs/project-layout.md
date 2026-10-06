# 项目目录地图（按用途分类）

> 2026-10-06 建立。**找东西先看这张表** —— 它是「哪个目录属于哪一类、能不能动」的唯一说明。
> 配套：仓库卫生红线见 [ai-spec.md](./ai-spec.md) §8.3；构建/发布流程见 §8.2。

## 一、八类落点

| # | 类别 | 落点 | 入库 | 体积 | 说明 |
|---|---|---|---|---|---|
| ① | **开发依赖** | `mingw64/`、`paddle-ocr/`、`local-models/`、`node_modules/`、`app/node_modules/`、`vscode-extension/node_modules/` | ✗ | 752 MB + 296 MB + 148 MB | 本机工具链与依赖。**全部由脚本按需获取/安装，删了能重来**（`scripts/download-paddle-ocr.ps1`、`npm install`） |
| ② | **插件依赖** | `tuning-engine/`、`licenses/`、`release/deps/` | ✅ / ✗ | 200 MB(含 target) | `tuning-engine` 是**宿主链接的 DSP 引擎**（路径依赖）；`licenses/live2d-cubism-core/` 是随包分发的授权文件；`release/deps/` 是构建期落的二进制（librespot / Live2D Core） |
| ③ | **dev 后端** | `app/src-tauri/src/`、`core-agent/src/`、`scripts/` | ✅ | 小 | 宿主（Tauri/Rust）+ 自研 agent 内核（独立 crate）+ 构建发布脚本 |
| ④ | **dev 前端** | `app/src/`、`vscode-extension/src/` | ✅ | 小 | 主界面 + 内置插件源码；VSCode 扩展 |
| ⑤ | **dev 编译产物** | `app/src-tauri/target/`、`core-agent/target/`、`tuning-engine/target/`、`app/dist/`、`app/plugin-dist/`、`vscode-extension/out/` | ✗ | **~15.8 GB** | 占整个仓库 90% 以上体积。可随时删（下次构建重来） |
| ⑥ | **release 发布包** | `release/` | ✗ | 177 MB | `Lunac-<ver>-Setup.exe`、`latest.json`、`ext-plugins/`、`plugin-packages/`、`deps/` |
| ⑦ | **ai 规范文档** | `docs/`、`agent-templates/` | ✅ | 1.3 MB | `ai-spec`（规范正文/红线）· `code-rules`（预检清单）· `agent-ui-spec`（界面）· `agent-feature-backlog`（待办）· `agent-implementation`（已实现）· 本文件；`agent-templates/` 是随包分发的技能/工具/Modules 模板 |
| ⑧ | **参考学习目录** | `_reference/`（含 `_reference/core/`） | ✗ | 413 MB | 第三方参考源码。**整目录 gitignore**，不进发行链路 |

入库的顶层只有：`.gitignore` `LICENSE` `README.md` `package.json` `agent-templates/` `app/` `core-agent/` `docs/` `licenses/` `scripts/` `tuning-engine/` `vscode-extension/`。

## 二、dev 与 release 的对应关系

| 面 | dev | release（安装后） |
|---|---|---|
| 宿主 exe | `app/src-tauri/target/debug/lunac.exe` | `D:\Lunac\lunac.exe` |
| agent | `app/src-tauri/target/debug/agent.exe`（构建 lunac 时从 ② 平铺过来的快照） | `D:\Lunac\agent.exe` |
| 插件 | `app/src-tauri/target/debug/Modules/`（`build-plugins.ps1` 同步） | `D:\Lunac\Modules\` |
| 技能/工具 | `app/src-tauri/target/debug/skills|tools/` | `D:\Lunac\skills|tools\` |
| 配置 | `app/src-tauri/target/debug/config/` | `D:\Lunac\config\` |
| 用量账 | `app/src-tauri/target/debug/ModuleData/` | `D:\Lunac\ModuleData\` |

⚠️ **`agent.exe` 只在「构建 lunac」时才刷新**（见 ai-spec 规则 35）：只跑 `cargo build`（core-agent）而不重建 lunac，dev 用的仍是旧 agent —— 排查前先对时间戳。

## 三、硬约束：这些目录不能随便挪

| 目录 | 为什么不能动 |
|---|---|
| `app/` `core-agent/` | 路径**写死在** `scripts/*.ps1`、`tauri.conf.json` 的 `bundle.resources`、`vite.config.ts` 里 |
| `tuning-engine/` | `app/src-tauri/Cargo.toml` 的**路径依赖**；搬它要同步改 `Cargo.toml` + `Cargo.lock` |
| `*/node_modules/` | 必须与 `package.json` 同级（npm 解析规则） |
| `*/target/` `app/dist/` `app/plugin-dist/` | cargo / vite 按相对路径产出的固定位置 |
| `agent-templates/` | `build-release.ps1` 与 NSI 按名引用 |

**因此本仓库不做「按分类大搬家」**：收益只是看着整齐，代价是几十处路径引用 + 逐项回归。分类用本文件表达即可。

## 四、清理记录

**2026-10-06（A 档轻整理）**
- 删除调试残留：`testapi.txt`、`_t_in/out/err.txt`、`.tmp-zoom-test/`、空目录 `local-models/`、`plugins/`、废弃的 `ui/`（旧 UI 的 `win-unpacked` 产物）
- **私钥移出仓库**：`.ssh-local/`（含 `id_ed25519`）已迁移到 `~\.ssh\`；删前对两份副本做过 SHA-256 逐字节校验；`.git/info/exclude` 里那条本地 exclude 也随之移除
- 上游参考源码归档：仓库根 `core/` → `_reference/core/`（`.gitignore` 改为忽略 `_reference/`，并保留 `core/` 双保险）
- 装好的 release 侧：删掉插件更新留下的备份目录 `Modules\.old-1790942629946`（35.9 MB，10-01 的 music 0.9.12）与过期的 `config\ai.json.bak-*`
- dev 侧：测试插件 `Modules\lunac-plugin-test`、测试技能 `skills\telegram-bot-collect`、测试配置 `config\mcp.json` / `mcp-trusted.json` 一并清除
- **备份点**：`git tag pre-cleanup-2026-10-06`（仓库回滚用）；`D:\Lunac-backup-2026-10-06\`（私钥 + release 侧被删项）

**未做（需要时再单独评估）**
- `⑤ dev 编译产物` 共 ~15.8 GB：要腾盘就删 `*/target`、`app/dist`、`app/plugin-dist`（代价 = 下次构建变慢）。**不建议用 `git clean`**，它不会区分「产物」与「未入库的本地资产」。
- 本地依赖（①）归到 `_deps/`：会牵动 `.gitignore` 与 `download-paddle-ocr.ps1` 等引用，属 B 档。

## 五、常用操作入口

| 想干什么 | 命令 |
|---|---|
| 构建全部插件 | `powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1` |
| 打安装包 | `powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1` |
| 发 Release（自更新用） | `powershell -ExecutionPolicy Bypass -File scripts\publish-release.ps1` |
| 发插件到市场 | `powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1` |
| 用量账对账 | `powershell -ExecutionPolicy Bypass -File scripts\reconcile-usage.ps1 -Csv <平台导出的 cost CSV>` |
| 环境自检 | `powershell -ExecutionPolicy Bypass -File scripts\verify-git.ps1` |

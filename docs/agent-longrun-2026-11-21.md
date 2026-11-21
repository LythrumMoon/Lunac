# Agent 长对话训练（2026-11-21）

> 目的：直接拉起 Lunac 的 `agent.exe`（与桌面端同一套 stream-json 协议），连续跑 3 小时
> 多轮对话，覆盖**全栈代码、硬件设计、商品设计**三条线，检查对话流、工具循环、
> 上下文压缩与前端任务分节是否出错，并记录可落地的优化点。
>
> 本文是运行记录与结论；原始 NDJSON 日志落在
> `temp/agent-longrun-taskphase-20261121/logs/`（不进仓库）。

## 运行配置

| 项 | 值 |
|---|---|
| Agent | `core-agent\target\release\agent.exe`（本次提交的构建） |
| 模型 | `config\ai.json` 的 `model`（opencode.ai 兼容端点） |
| 协议 | `--input-format stream-json --output-format stream-json --include-partial-messages` |
| 审批 | 常驻模式无前端，因此 harness 对 `control_request(can_use_tool)` 回 `allow` |
| 工作区 | `temp\agent-longrun-taskphase-20261121`（独立目录，不碰仓库源码） |
| 时长 | 最多 3 小时；单轮上限 45 分钟 |
| 分节 | 每轮显式要求 `TaskPhase start/done` + `TodoWrite` |

### 为什么用 harness 回审批

`--dangerously-skip-permissions` 只在**单发**（`--print`）模式生效；常驻模式（宿主的
stdio 流式模式）由前端审批，没人回 `control_response` 时写类工具会干等 300 秒后失败。
第一次试跑就是这样：`Cmd` / `Write` 全部 300 秒超时，10 分钟只产出 188 个 token。
harness 现在对所有 `can_use_tool` 自动回 `allow`，其余 subtype 一律 `deny`。

## 对话脚本（按领域交替）

1. 全栈：本地优先的个人知识库 + RSS 订阅（Rust/axum/sqlite + 纯 HTML/JS），含数据模型、
   REST、增量同步、FTS5、OPML、测试。
2. 硬件：ESP32-S3 环境监测网关（温湿度/CO2/PM2.5/光照/噪声），含 BOM、电源预算、
   PCB、固件任务、校准与产测、EMC/ESD。
3. 商品：桌面 AI 助手 + 环境网关的画像、JTBD、竞品、定价、GTM、30/60/90 指标、PRD。
4. 交叉：三者一致性、决策日志、端到端失败分支、性能剖析、最终复盘。

## 结果

### Session A：领域覆盖跑（2026-11-21 13:04–13:46，42.5 分钟）

| 指标 | 值 |
|---|---|
| 完成回合 | 3 / 4（第 4 轮进行中被外部 `Stop-Process node` 误杀） |
| 工具调用 | 330（错误 4） |
| TaskPhase | start 16 / done 15；4/4 回合都用了 |
| TodoWrite | 19 |
| 上下文压缩 | 4 次 |
| Token | 输入 403,669（缓存读 20,830,848）/ 输出 145,203 |
| 预估费用 | ¥1.40 |
| 回合耗时 | p50 530.6s / p95 975.2s |

逐轮：

| # | 领域 | 秒 | 工具 | 错误 | TaskPhase | 预览 |
|---|---|---|---|---|---|---|
| 1 | 全栈 | 975 | 140 | 1 | 4/4 | 本地优先知识库 + RSS（Rust/axum/sqlite + HTML/JS） |
| 2 | 硬件 | 531 | 42 | 1 | 4/4 | ESP32-S3 环境监测网关（BOM/电源/PCB/固件/产测） |
| 3 | 商品 | 489 | 47 | 0 | 4/4 | 桌面 AI 助手 + 网关的画像/定价/GTM/PRD |
| 4 | 交叉 | — | 101 | 2 | 4/3 | 三者一致性检查（被外部误杀） |

工具错误：

| 工具 | 次数 | 原文（第一条） |
|---|---|---|
| Edit | 3 | `old_string not found in project/src/opml.rs` |
| WebBrowser | 1 | `https://127.0.0.1:18080/ → empty DOM` |

### 发现

1. **无人值守的常驻模式不会因为 `--dangerously-skip-permissions` 自动放行**  
   该开关只在单发（`--print`）模式生效；常驻模式必须由前端回 `control_response`。  
   第一次试跑 `Cmd`/`Write` 全部干等 300 秒后失败，10 分钟只产出 188 token。  
   → harness 现在对**非危险** `can_use_tool` 回 `allow`，其余 `deny`。

2. **模型的清理命令会按进程名批量杀进程**  
   第 4 轮它执行 `Get-Process node | Stop-Process -Force`，把 harness 的 node 也杀了，
   长跑在 42 分钟处中断。静态安全分析其实已标出 `dangerous: ["强制结束进程"]`，  
   是 harness 无脑 allow 才放行。  
   → harness 改为拒绝所有 `analysis.dangerous` 非空的审批；进程名也换成
   `lunac-harness.exe`，避免被 `node` 名字批量命中。  
   → 产品侧结论：前端「危险命令必须人工确认」这条不能省；自动化调用方必须自己
   遵守同一个 `analysis.dangerous` 判定。

3. **TaskPhase 采纳率**：4/4 回合使用了 TaskPhase（16 start / 15 done），
   说明显式提示词有效。每个回合的**第一段正文仍早于第一个 start**（4/4）；  
   前端现在会把这段正文并进第一个任务条（`retitleSection`），用户看到的结构仍是
   「任务条 → 命令 → 小总结」。下一版可把「先 start 再说话」再往系统提示词前面挪。

4. **上下文压缩 4 次**：任务快照（TodoWrite）被正确钉回，没有出现「清单消失」；
   但也说明 140 工具调用的长回合会很快撞到上下文水位。长任务应更早给阶段小总结，
   让压缩丢的是过程而不是结论。

5. **长回合**：p95 975 秒，第 1 轮全栈实现接近 16 分钟；期间模型大量重复读文件
   （140 次工具）。可优化：Edit 失败时返回最近似位置提示，减少「读整文件 → 猜
   old_string → 失败 → 再读」的循环。

6. **本地 WebBrowser 空 DOM**：测试服务器刚起就抓页面，拿到空 DOM。  
   可优化：WebBrowser 对 `127.0.0.1` 第一次空 DOM 做一次短重试（或等待 `load` 事件），
   而不是直接返回失败。

### 已落地的修正

- **任务分节**：新增默认工具 `TaskPhase`（`start` / `done`），前端固定
  `小待办任务 → 命令 → 小总结输出 → … → 大总结输出 → 产出物`。
- **标题冻结**：TodoWrite 改名不再改上一次任务条的名字；改名会开新阶段。
- **小总结位置**：阶段最后一条正文固定渲染在任务条下方，不再漂到条上方。
- **产出物**：从任务条里移出，固定跟在大总结之后。
- **历史压缩**：被裁掉过程的回合仍保留任务条，并标注「过程记录已压缩」。

### 根据报告已落地的优化（2026-11-21）

| 报告问题 | 修改 | 验证 |
|---|---|---|
| `Edit` 失败后整文件重读 | `Edit` 的 `old_string not found` 现在带**最近似行号 + 片段**（`nearest_line_hint`） | core-agent `edit_hint_points_at_the_closest_line` |
| `WebBrowser https://127.0.0.1:18080/ → empty DOM` | 根因是 `normalize_url` 把本机 http 盲升 https；现在 localhost / 127/8 / 10/8 / 172.16/12 / 192.168/16 / `.local` 保持 http | `normalize_url_keeps_local_http_and_upgrades_public_hosts` |
| 常驻无审批通道时写类工具干等 300 秒 | 常驻 + `--dangerously-skip-permissions` + 无 `--permission-prompt-tool` ⇒ 自动按无人值守放行；有 stdio 通道时仍由前端审批（桌面路径不变） | `cargo test` 512 passed；启动日志有明确 warn |
| 长上下文压缩可能丢 TaskPhase 小总结 | `classify_message_layer` 把 `TaskPhase` 与 `TodoWrite` 一样判 L3（压缩时优先保留） | `classify_message_layer` 的 TaskPhase 断言 |
| 第一段正文早于 TaskPhase start | 系统提示词改成「多步回合的**第一个动作**就是 TaskPhase start（不许先说话）」；前端仍保留合并兜底 | 提示词 + 已有 `preamble ... merges into that bar` widget test |

`Edit` 的新错误形态：

```text
old_string not found in <path>; closest line 42: let value = compute();
```

### 优化后的端到端验证

```text
# 1. 常驻无审批通道（新 build，去掉 --permission-prompt-tool stdio）
prompt: 在工作区 verify.txt 写一行 ok
result: duration 7.35s / toolUses 2 / toolErrors 0
verify.txt = ok

# 2. Edit 近似提示
call : Edit old_string = "let  value = compute();"（两个空格，故意不匹配）
error: old_string not found in ...\target.txt; closest line 2: let value = compute();

# 3. 本机 http 不再盲升 https
call : WebBrowser http://127.0.0.1:18081/
result: toolErrors 0（页面 hello-local）

# 4. TaskPhase 顺序（新 build）
first assistant message blocks: ["thinking", "TaskPhase", "TodoWrite"]   # 没有正文抢先
actual tool sequence:
  TaskPhase start "写 p1.txt"
  TodoWrite  (p1 in_progress)
  Write      p1.txt
  TaskPhase done "已在工作区写入 p1.txt..."
  TodoWrite  (p1 completed, p2 in_progress)
  TaskPhase start "写 p2.txt"
  Write      p2.txt
  TaskPhase done "..."
  TodoWrite  (p2 completed)
```

# 4. 测试与构建
core-agent cargo test --offline : 512 passed / 0 failed / 16 ignored
release build（--target-dir target-verify）: Finished，0 warning
```

### Session B：3 小时连续跑（2026-11-21 21:48 – 2026-11-22 00:48）

| 指标 | 值 |
|---|---|
| 墙钟时长 | **3.00 小时**（结束原因 `deadline`） |
| 完成回合 | 15 / 15 |
| 工具调用 | 1,242（错误 21） |
| TaskPhase | start 53 / done 60；15/15 回合都用了 |
| TodoWrite | 46 |
| 上下文压缩 | 29 次 |
| Token | 输入 4,610,616（缓存读 399,114,880）/ 输出 599,352 |
| 预估费用 | ¥14.99 |
| 回合耗时 | p50 544.3s / p95 1707.8s |

逐轮：

| # | 秒 | 工具 | 错误 | TaskPhase | TodoWrite |
|---|---|---|---|---|---|
| 1 | 366 | 62 | 1 | 3/4 | 4 |
| 2 | 1153 | 130 | 7 | 4/4 | 5 |
| 3 | 425 | 64 | 0 | 3/3 | 4 |
| 4 | 350 | 52 | 0 | 4/4 | 4 |
| 5 | 544 | 48 | 0 | 4/4 | 3 |
| 6 | 270 | 34 | 0 | 3/3 | 4 |
| 7 | 849 | 114 | 0 | 3/4 | 3 |
| 8 | 1416 | 137 | 0 | 2/4 | 3 |
| 9 | 299 | 39 | 0 | 4/4 | 1 |
| 10 | 1708 | 196 | 5 | 5/4 | 2 |
| 11 | 751 | 93 | 1 | 4/4 | 3 |
| 12 | 431 | 60 | 2 | 4/4 | 2 |
| 13 | 478 | 48 | 0 | 4/5 | 3 |
| 14 | 632 | 74 | 1 | 4/5 | 2 |
| 15 | 1101 | 91 | 4 | 2/4 | 3 |

错误分布：

| 工具 | 次数 | 说明 |
|---|---|---|
| Edit | 8 | `old_string not found`（本轮跑的是带 hint 之前的 build） |
| PowerShell | 6 | harness 拒绝危险命令（`User denied this action`），是安全闸生效，不是 agent 故障 |
| WebFetch | 6 | 连续猜过期的厂商文档 URL：404 ×5 / 403 ×1 |
| WebBrowser | 1 | 数据手册 PDF 渲染成空 DOM |

Session B 之后新增的修正：

1. `WebFetch` 对 401/403/404/410 的错误里附上
   `use WebSearch to find the current address first`，不再让模型连续换 URL 猜。
2. `WebBrowser` 对 `.pdf` 的空 DOM 明确提示「PDF 不会被渲染成 DOM，下载后用本地读取」。
3. `TaskPhase done` 没有对应 `start` 时，前端不再丢弃总结：以用户提问为标题补一个任务条
   （Session B 里出现 5 次）。
4. Session B 用的是加「first action = TaskPhase start」之前的二进制；新 build 的端到端
   验证已经看到首个 assistant 块为 `[thinking, TaskPhase, TodoWrite]`，没有正文抢先。
5. `Edit` 最近似行提示、`TaskPhase` L3 压缩保留、本机 http 不升 https 都在 Session B
   之后提交，尚未在长跑里复测。

## 复现

```powershell
# 1. 构建本次 agent
cd core-agent
cargo build --release --offline

# 2. 拉起 3 小时训练（需要 config\ai.json 已配置）
node <harness>\run_long.mjs --hours 3 --turn-minutes 45 `
  --agent <repo>\core-agent\target\release\agent.exe `
  --workspace <repo>\temp\agent-longrun-taskphase-20261121

# 3. 运行结束后生成报告
node <harness>\analyze_long.mjs <events.ndjson> [summary.json] <report.md>
```

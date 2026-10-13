# Agent 待办清单——已完成 / 已闭环历史归档

> **本文档是什么**：`agent-feature-backlog.md` 中**已完成 / 已闭环**条目的压缩归档。每条只留「编号 + 标题 + 一句话结论 + 验证码 / 证据」，**不再保留逐条过程**（过程与实测数据的完整版见 Git 提交记录与 `docs/archive/ai-spec.md` §11 规则）。
>
> 建立于 2026-10-11 文档整合。**新规则一律写进 `docs/` 根下那 6 份现行文档，不要再回写本文件。**

---

## 一、桌面宿主 / 性能类事故（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| M2-10 | 聊天窗整段会话渲染成 DOM ⇒ renderer 3.87 GB | 聊天流窗口化渲染（`CHAT_RENDER_WINDOW=120`）+ 动作条懒创建 + 提问节点窗口化 | CDP 实测 45,216 条消息 → 1,088,429 节点 / 3.87 GB；规则 76 |
| M2-12 | `chat.db` 298 MB / 一份会话 4.5 万条 | 根因 = `pruneContext()` 数组别名（`turns.push(current)` 忘了 `current = []`），一行修复 | 探针 `26→312→3744→44928→539136`，修复后 `26→24→24`；库去重重写 **298.54 MB → 0.81 MB**；规则 77 |
| M2-13 | 代理插件落地（系统代理 + librespot 联动） | `system_proxy.rs` + `librespot_set_proxy` + `proxy.ts`（多页）接上；`-x` 收进 `librespot_args()` | 已发布 6 条（`gh api` 核实）；§4.11；预检 #46 |
| M2-14 | 插件子进程必须随插件退出而终结 | `child_job.rs` 提为共用模块 + 每个 `spawn()` 后 `assign` + `stop_cli` 走 `taskkill /F /T` | `cargo test --bins` 178 passed；预检 #57 / 规则 78 |
| M2-9 | 音乐插件长连接断开后 `404 NO_ACTIVE_DEVICE` | 控制命令**不带** `device_id`（对准活跃设备），吃 404 则 `transfer_to(play:false)` 扶正后重发一次；Spotify 429 = 端点级长处罚 → 少打 + 按 `Retry-After` 停够 + 保留手动恢复 | §4.6「Spotify Web API 限流」；预检 #62 / #63 |
| M2-16 | 缓存 miss 归因：71 万 token 来自后台命令输出无预算 | A–E 五条全落地（含 D：往期索引 / 记忆块移出 `system`、改注 history 开头） | `cargo test --bins` 147 passed；规则 18 / 22 / 23 / 53 / 56 / 70 |
| M2-17 | 新建对话与上个对话「模型级隔离」 | 彻底移除「往期会话索引」自动注入；三处新会话入口清 `pendingAgentHistory` | core-agent `cargo test` 148 passed；规则 53 / 56 |
| M2-18 | 「AI 自建东西该放哪」四分法 | `env_block()` 新增 `placement_line`（插件→`Modules\`、技能→`skills\`、工具→`tools\`、其它→工作目录） | core-agent 148 passed；§20.1 |
| — | 权限三档改名 | `项目/完全` → `工作区/系统`（差别 = 是否受工作区限制）；`full` 仍关掉工作区锁（刻意保留） | i18n 5 语言；规则 21 |
| — | 代理插件前端重构 | 4 页（节点/订阅/日志/设置）+ 自制分段控件（消灭原生 select 与 number spinner）+ 去解释文案 | `tsc` / `vite build` / `build-plugins -Plugin proxy` 通过 |

## 二、Agent 能力与协议（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| A13 扩展 第 1–4 步 | 项目级 MCP（`.mcp.json`，http + stdio） | stdio 传输泛化 + 读 `<cwd>\.mcp.json` + 信任门 + 推迟到首次查询前连接 + 前端信任卡 | 3 条单测；§20.1；规则 18 |
| A13 扩展 第 6 步 | 审批卡 / 计划卡「一大段文档」 | 真凶是计划卡 ⇒ 长计划默认折叠（`PLAN_PREVIEW_LINES=12`），**不用摘要替换原文** | agent-ui-spec §9；规则 68 |
| A13 扩展 第 5 步 | AI 运行时自建技能 | 工作区锁对 `<exe 根>\skills\**` 开**受审批**的例外（只放行、不代替审批） | 单测 `skills_dir_is_the_only_workspace_lock_exception`；规则 67 |
| — | MCP 配置改动检测 + 自动重启（两版） | agent 检测（空闲轮 `recv_timeout(1s)` + 事件带 `idle`）、前端执行；空闲立即重启、回合中等 `result`；只报一次 | 单测 `config_watch_reports_once_and_covers_new_files`；`cargo test --bins` 146 passed；规则 69 |
| — | MCP 其余能力 | Streamable HTTP / OAuth 2.1 / prompts / roots / elicitation 全部落地（协议升 `2025-06-18`） | 规则 55 / 84；`mcp_oauth.rs` |
| — | 内置工具与条件注册 | `defs()` 恒 **19 件**（17 件 + `PetControl`（2026-10-10）+ `NotebookEdit`（2026-10-11））；`Skill` / resources×2 / prompts×2 / `Remember` / `ImageGen` / `mcp__*` 条件注册 | 守门单测 `agent_tool_is_registered_and_filterable` |
| — | 权限 hooks（A9） | 8 事件介入；只认显式拒绝；hook 的 `allow` 只跳过审批卡；mtime 热重载 | 规则 61 |
| — | 结论：长期记忆「从未写入」 | 非缺失——`Remember` 靠模型主动调、复盘按 10 次提问触发且计数随重启清零 ⇒ 门槛够不着（是否调低待定） | 2026-10-07 观察 |

## 三、调音引擎与音乐插件（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| L5 S0 | DSP 引擎内核 | `tuning-engine/` 独立 crate + 七模块 + 四子命令 CLI；「实测增益 = 理论 ±0.1dB」 | 54 条单测；端到端 `+5.94dB = +5.94dB`；§4.10 |
| L5 S1 | 自身播放链路闭环 | 引擎以**库依赖**进宿主，套在本地播放链上；默认关闭（opt-in） | §4.6「调音」；预检 #51 |
| L5 S2/P4 | WASAPI loopback 闭环测量 | 两趟（直通量系统 / 走链量最终）相除得到「仅自身链（实测）」；面板画三条线 | 单测 `two_pass_subtraction_isolates_the_chain_from_the_system` |
| L5 P1–P5-7 | 调音页对齐 Peace | 可编辑链 + 数字界面 + 频响曲线独立窗 + 扬声器声道槽 + GraphicEQ / 卷积 / 延迟 / 复制 + 预设管理 + 默认档改 flat + 曲线反向同步 | 引擎 `cargo test` 96 绿、宿主 193 绿、`tsc` / build 通过 |
| L5 P7 | 在线音乐也进调音 | librespot `-B pipe -f F32` + `live_audio.rs`（无锁 SPSC ring）把裸 PCM 喂进**同一个** `TuningSource` | 宿主 `cargo test --bins` 199 绿 |
| L5 剩余 | A-B 盲测 | 两个快照槽 + 随机映射 + 揭晓；随机只在前端、不跨挂载；**不做自动等响度** | 实机验收通过；规则 73 / 预检 #73 |
| L9 | 音乐插件剩余项 | ①媒体库（`media_lib.rs` + `lofty`）②本地播放（`player.rs`，`rodio`）③播放列表解析 全部落地；**连接 QQ / 网易云经用户决定取消** | §4.6「本地媒体库」；预检 #47 / #49 |
| L7 | AI 聊天独立界面（第一版） | `pluginId="chat"`，加载 `index.html`，1280×720；主窗内嵌 / detached 两态去掉 | 宿主 `cargo test --bins` 112 passed |

## 四、成本与用量（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| L6 归因 | 「Lunac 金额是 Trae 10 倍」 | 结案：**不是计价贵**——11.2 倍 = 命中率差 3.09× × token 结构差 3.63×（官方峰谷定价）；KPI 改用「每次提问成本」 | §9.1 难点 3 |
| L6 六件 | 降本措施 | 子代理 / 复盘用量并入 `result.usage`、价格表时段价 + 预置峰谷价、输出瘦身（成本 -41%）、对账基线工具 | §3.5；规则 63 |
| L6 跨源基线 | 真实账单对账 | 修掉两个真问题：① 节假日全天按谷价（修正 0.979 元）② 本地账漏记没跑完的回合；**验收口径 = 差额 0.000000、exit 0** | `reconcile-usage.ps1`；规则 87 |
| 规则 88 | 输出 token 是第一杠杆 | 输出占账单 61.7%；① 改已存在文件用 `Edit`（Write 调用 → 0）② 思考是最大单项且不回灌 ③ `max_tokens` 是三合一池（已抬 `THINKING_HEADROOM_TOKENS`） | 平台用量 + 自建 A/B 实测 |

## 五、桌宠 × AI（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| L1 | Live2D 桌宠本体 | 独立透明置顶窗 + 引擎（`pixi.js@7` + `pixi-live2d-display-lipsyncpatch`）+ Core 自托管（带版本 + sha256 + 授权文件）+ 发布 | 真机渲染 18215 非透明像素；§4.8 |
| L1-A/B | 桌宠三条裁决 | 形态=完全独立窗（第二份 WebView2，边际 +1 renderer / 工作集 +109 MB）；模型由用户自备；实施顺序不动 | architecture-rendering §6.2 |
| M2-19 | 桌宠 × AI（① ② ③） | 手动抓屏 `screenshot_monitor` + 「看一眼屏幕」；对话框 + 主动反应（默认关）；用量来源 `source="pet"` + 成本面板来源下拉 | 契约 `pet-chat-send` / `pet-chat-reply`；§4.5 |
| M2-20 | 桌宠穿透开关对齐 + 专属人格 | 穿透改切换按钮 + 双向同步；`config\pet-persona.md` 按轮注入（即时生效、不重启） | §4.5 |
| M2-21 | 音乐插件队列 / 位置 / 视觉 / curl 导入 | 队列页 + 自定义队列 + 播位恢复 + 警告区不占位 + 拖动条减半 + curl 一键导入 | `tsc` exit 0；`build-plugins -Plugin music` |
| M2-22 | 桌宠 × AI 第二轮（多批） | 对话独立成线程 + 历史分页；间隔可自定义；默认人格；自动弹窗；TTS 语音朗读；`thinking` 按轮关；`emit_to` + `getCurrentWindow().listen`；热键改 AI 触发 + 默认键；表情可叠加开关 + 三方参数合并 | `tsc` exit 0；`build-plugins -Plugin pet`（v0.9.31） |

## 六、界面与交互（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| agent-ui-spec 阶段 1/2 | 对话区改造 | 思考块规范化 + 命令卡片 + `cli-stderr` 落 `.sys-note-error` + 回合自动折叠 + 运行方式三档 + 安全档位入口 + 越界卡片三动作 | agent-ui-spec §10 阶段 1/2 已落地 |
| U1 | 代码变更 diff 卡 | **事后审阅**（接受=标记已阅 / 拒绝=写回快照）；三级粒度；diff 由 `text-diff.ts` 有界重算 | `tsc` / `npm run build` exit 0；规则 86 |
| U2 | 多轮缩略导航 | 历史抽屉可摊成「轮次缩略」，点某轮 = 恢复并跳到那一轮；会话 Fork 取消 | `tsc` / `npm run build` exit 0 |
| M2-15 | 待办落盘 + 审批卡停靠 + 提示音 | 待办按回合存 `chat.db`；审批 / 计划卡挂 `#approval-dock`；提示音走宿主 `MessageBeep` | 规则 79 / 80 / 81；`cargo test --bins` 180 passed |
| M2-6 | 中断后飞行中命令卡收尾 | 三处调 `markFlyingCardsStopped`（停止 / closed / 回退）；只改视觉、不置 `card.done` | 规则 75 |

## 七、工程 / 发布 / 卫生（闭环）

| 编号 | 标题 | 一句话结论 | 证据 / 验证码 |
|---|---|---|---|
| §8.2 | 打包流程收敛 | 九步 → 八步；PaddleOCR 移出安装包（改 `ocr` 插件 `archive` 依赖，110.3 MB → 15.9 MB）；`ext-plugins` 用**新鲜度**守卫 | §8.2；`cargo` / 7z 校验 |
| §8.3 | 仓库卫生 | `_reference/core/` 归档 + gitignore；`.env` 不入库；大二进制不入库；`cli.exe` 不进包（打包前后双校验） | §8.3 红线 |
| L12 | 插件体系扩展 | ① 插件可自带 UI 框架写进 README ② sidecar 契约定稿并实施（宿主代请求 + 独立信任门 + 面板绑定） | `cargo test --bins` 206 passed；规则 71 |
| — | BOM 纪律 | `.ps1`/`.nsi` 恰一份 BOM 的判据与归一 + `commit.ps1` 自动愈合 + `npm run verify` 第 ⑧ 节守卫 | 规则 82；预检 #61 |
| — | 旧 CLI 差距面 | `getAllBaseTools()` 每个名字已归位（实现 / 待办 / 设想）；旧编号 `backlog §0–§8.x` 对应条目**全部完成并从清单删除** | backlog §5 边界声明 |

---

> **本归档不承载规则**：规则正文见 `docs/` 根下 6 份现行文档；历史过程见 `docs/archive/ai-spec.md` §11 与 Git 记录。


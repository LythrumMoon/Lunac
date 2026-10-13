# docs 文档索引与新旧映射

> **本文档是什么**：`docs/` 的**总入口与映射表**。① 6 份现行文档各自管什么、什么场景读哪份；② 旧文档的章节 / 编号现在落在哪份新文档；③ 源码里对旧编号的引用指向哪。
>
> 建立于 2026-10-11（文档整合，详见文末「整合记录」）。

## 一、现行文档（`docs/` 根，共 6 份）

| 文档 | 管什么 | 什么时候读 |
|---|---|---|
| [`code-standards.md`](./code-standards.md) | 代码硬规则：分层 / 命名 / 单一真相源 / TS 与 Rust 约定 / 前后端契约 / WebView2 与 CSS 陷阱 | 写 / 改任何代码前 |
| [`forbidden.md`](./forbidden.md) | **禁止事项**（最高优先级）：只列「绝不允许」的硬禁令 + 每条一句为什么 | 任何改动前的第一道速查 |
| [`testing.md`](./testing.md) | 构建 / 检查 / 验收链：`tsc` / `cargo` / `verify` / 打包、BOM 纪律、预检清单、真机验收、调试 / 审查 / TDD | 提交前、打包前、验收时 |
| [`logging.md`](./logging.md) | 日志位置 / 分级 / 写什么不写什么 / 脱敏 / 诊断打点与排查套路 | 加日志、排查线上问题时 |
| [`todo.md`](./todo.md) | **待办（未完成 + 未验证）**，每条带调研结论 | 想找「还剩什么要做」时 |
| [`ui-style.md`](./ui-style.md) | 界面与前端风格：色板 / 变量 / **所有按钮与态** / 圆角间距 / 图标 / 弹层 / 滚动条 / 选中禁用 hover / 主题明暗 / **自定义颜色四类**（§界面 11） | 写 / 改界面样式前 |

> 另有 `docs/parse-minidump.ps1`（**脚本不是文档，未动**）。

## 二、历史归档（`docs/archive/`，旧文件保留不删）

| 文件 | 内容 |
|---|---|
| `ai-spec.md` | 项目规范正文（协议契约 §3.5、硬约束规则 §11 规则 1–95 等） |
| `agent-feature-backlog.md` | 待办清单原文件（含 §0 闭环记录、§4.1 长期规划全文、§5 边界声明） |
| `code-rules.md` | 预检清单 #1–#77 与反模式表 |
| `agent-ui-spec.md` | 对话区界面规范全文 |
| `icon-style.md` | 图标风格规范全文 |
| `architecture-rendering.md` | 渲染层架构决策（ADR） |
| `agent-implementation.md` | agent 实现现状与本地扩展格式 |
| `project-layout.md` | 项目目录地图 |
| `agent-feature-backlog-history.md` | **本次新建**：backlog 中已完成 / 已闭环条目的压缩归档 |

## 三、新旧映射表（`ai-spec.md` 章节级）

> 「未迁移」= 该内容是**协议契约 / 实现细节 / 历史记录**，6 份现行文档未承载，**以 archive 原文件为准**（不是废弃）。「已废弃」= 内容已作废或被取代。

| 旧位置 | 现在在哪 |
|---|---|
| §1 架构概述、§5 关键设计决策、§6 文件结构、§18 插件发现架构 | → `code-standards.md` §规范 1 |
| §2.1 前端搜索启动器、§2.4 窗口配置 | → `ui-style.md` §界面 9 + `code-standards.md` §规范 8 |
| §2.1.1 i18n | → `ui-style.md` §界面 10 |
| §2.1.2 详细搜索大界面 | → `ui-style.md` §界面 9 |
| §2.2 后端 Rust 原生功能 | → `code-standards.md` §规范 5 |
| §2.3 热键双后端 | → `code-standards.md` §规范 8 |
| §3.1–3.4 agent 架构 / 技术实现 | → `code-standards.md` §规范 6（架构性约定） |
| §3.5 自研 agent 协议契约（stream-json 字段与语义） | **未迁移**（协议唯一契约，archive 保留） |
| §3.6 数学公式渲染 | 未迁移 |
| §3.7 Trae 参考规范 | **已废弃**（参照点，实施完毕后不再是规范） |
| §4.1–4.4 插件系统 / Agent Tools / 状态保存 / 搜索匹配 | → `code-standards.md` §规范 1 / 2（其余细节未迁移） |
| §4.5 插件清单 | 未迁移 |
| §4.6 音乐插件（歌词 + Spotify）、§4.10 调音引擎 | → `todo.md` §待办 1-5（剩余）+ archive |
| §4.7 文件转换插件、§4.9 翻译插件 | 未迁移 |
| §4.8 插件悬浮窗 | → `code-standards.md` §规范 5 / 8（其余细节未迁移） |
| §4.11 代理插件 | → 已闭环，见 `archive/agent-feature-backlog-history.md` |
| §7 开发命令、§8.1 版本号、§8.2 打包流程 | → `testing.md` §测试 1 / 2 |
| §8.3 开源发布 / 仓库卫生 | → `forbidden.md` §禁用 5 |
| §8.4 应用内自更新 | → `todo.md` §待办 1-3 |
| §9 当前问题 / §9.1 疑难点 | 未迁移（结论已并入 `todo.md` / archive） |
| §10 待办的落点 | → `todo.md`（+ 本文档） |
| §12 Agent Plan 模式规范（给模型看的计划写法） | 未迁移（archive 保留） |
| §13.1 文件内容级静态安全扫描 | → `forbidden.md` §禁用 1 / 6 |
| §13.2 会话文件清理服务（设想，不做） | **已废弃**（便携式 temp 整目录即清理单位） |
| §14 系统化调试规范 | → `testing.md` §测试 7 |
| §15 代码审查工作流 | → `testing.md` §测试 8 |
| §16 TDD 开发规范 | → `testing.md` §测试 9 |
| §17 AI 文本去痕迹（Humanizer） | **已废弃**（2026-09-30 整体移除） |
| §19 Hermes 前端接入 / §19.1 调试阶段状态栏 | → `todo.md` §待办 3-4（L4） |
| §20 用户自定义工具 / 技能 / §20.1 | → `code-standards.md` §规范 1 / 2 |

## 四、新旧映射表（`ai-spec.md` §11 规则级，1–95）

| 规则 | 主题 | 现在在哪 |
|---|---|---|
| 1 | 开机自启 | 未迁移（archive） |
| 2 | 设置·AI 供应商 / 模型 | `code-standards.md` §规范 2 |
| 3 | 设置·技能扩展 | `code-standards.md` §规范 1 / 2 |
| 4 | 搜索性能（唯一存储） | `code-standards.md` §规范 2 |
| 5 | 通用自定义下拉框 | `ui-style.md` §界面 6 · `forbidden.md` §禁用 2 |
| 6 | 搜索引擎预设 | 未迁移 |
| 7 | 数据目录 | `code-standards.md` §规范 1 |
| 8 | 备忘录插件 | 未迁移 |
| 9 | 图标风格 | `ui-style.md` §界面 4 / 5 |
| 10 | 文件附件省略折叠 | `ui-style.md` §界面 9 |
| 11 | 自定义文件启动 | 未迁移 |
| 12 | 卸载清理 | `forbidden.md` §禁用 5 · `testing.md` |
| 13 | OCR 引擎按需下载 | `testing.md` §测试 2 |
| 14 | Agent 内置工具与审批 | `forbidden.md` §禁用 1 · `code-standards.md` §规范 2 |
| 15 | 上下文压缩不变量 | 未迁移（契约） |
| 16 | 测试用 flash 模型 | `testing.md` §测试 6 |
| 17 | MCP 工具命名与审批 | `code-standards.md` §规范 3 · `forbidden.md` §禁用 1 |
| 18 | 前缀缓存不变量 | `code-standards.md` §规范 6 |
| 19 | 用量口径不变量 | `code-standards.md` §规范 6 |
| 20 | 落盘日志 | `logging.md` §日志 1 / 2 |
| 21 | AI 对话面板硬约束 | `ui-style.md` §界面 6 / 9 · `forbidden.md` §禁用 2 |
| 22 | 系统提示词环境块 | 未迁移（契约） |
| 23 | 缓存命中率解释口径 | `logging.md` §日志 5 · `code-standards.md` §规范 6 |
| 24 | Agent 能力定位与本地扩展目录 | `code-standards.md` §规范 1 |
| 25 | 瞬时失败重试 | 未迁移 |
| 26 | 命令静态安全分析 | `forbidden.md` §禁用 1 |
| 27 | 单条工具输出预算 | `code-standards.md` §规范 6 · `forbidden.md` §禁用 7 |
| 28 | 只读工具并行 | 未迁移 |
| 29 | 详细搜索大界面 | `ui-style.md` §界面 9 |
| 30 | 会话历史回灌与存储 | 未迁移（契约） |
| 31 | 唤出路径顺序不变量 | 未迁移 |
| 32 | 被改动文件路径追踪 | `ui-style.md` §界面 9 |
| 33 | `pre-wrap` 幽灵空行 | `code-standards.md` §规范 7 · `forbidden.md` §禁用 2 |
| 34 | agent 默认工作目录 | `code-standards.md` §规范 1 |
| 35 | dev 布局 agent 快照 | `testing.md` §测试 4 · `logging.md` §日志 6 |
| 36 | WebView2 profile 留痕 | `logging.md` §日志 5 / 6 |
| 37 | 任务快照 | 未迁移（契约） |
| 38 | 环境变量合并语义 | `forbidden.md` §禁用 4 · `code-standards.md` |
| 39 | 摘要式压缩 | 未迁移（契约） |
| 40 | `show_and_focus` 覆盖 | 未迁移 |
| 41 | 系统动作 `action_spec()` | `code-standards.md` §规范 2 |
| 42 | 详细搜索检索上限 | `ui-style.md` §界面 9 |
| 43 | 日志分级与降噪 | `logging.md` §日志 3 |
| 44 | 结果区高度由行数决定 | `ui-style.md` §界面 9 |
| 45 | 外观 / 主题（CSS 变量） | `ui-style.md` §界面 1 / 8 / 11 |
| 46 | 主题包锁定 | `ui-style.md` §界面 8 |
| 47 | 搜索结果呈现与容错 | `ui-style.md` §界面 9 |
| 48 | 中性叠加 token | `ui-style.md` §界面 1 / 8 |
| 49 | UI 文案不得内嵌 emoji | `ui-style.md` §界面 5 · `forbidden.md` §禁用 2 |
| 50 | 插件名 i18n + 设置分类 | `ui-style.md` §界面 9 / 10 |
| 51 | 新增按钮必须显式主题样式 | `ui-style.md` §界面 2 |
| 52 | 文档结构纪律（待办落点） | `todo.md` + 本文档 |
| 53 | 往期会话检索（禁止自动注入） | `forbidden.md` §禁用 4 |
| 54 | 子代理框架 | 未迁移（契约） |
| 55 | MCP 读侧（resources / prompts） | 未迁移（契约） |
| 56 | 长期记忆与后台复盘 | `todo.md` §待办 3-8 · 未迁移（契约） |
| 57 | 技能（fork 模式 / 脚本资源） | `code-standards.md` §规范 1 · 未迁移 |
| 58 | 写入内容凭据扫描 | `forbidden.md` §禁用 1 / 6 |
| 59 | 计划模式闭环 | 未迁移（契约）· `forbidden.md` §禁用 1 |
| 60 | 图片附件 | 未迁移（契约） |
| 61 | 权限 hooks | `forbidden.md` §禁用 1 |
| 62 | 只读分类 | `forbidden.md` §禁用 1 |
| 63 | 定价表与成本面板 | `code-standards.md` §规范 2 · `todo.md` §待办 1-2 |
| 64 | 会话 id 与 rewind | 未迁移（契约） |
| 65 | 一轮多子任务并行 | 未迁移（契约） |
| 66 | 用户人格 / 自定义提示词 | 未迁移（契约） |
| 67 | 第三方插件市场 | `forbidden.md` §禁用 5 |
| 68 | 宿主命令不许阻塞 IO | `code-standards.md` §规范 5 · `testing.md` §测试 4 |
| 69 | 翻译插件 | 未迁移 |
| 70 | 待发送队列 | 未迁移 |
| 71 | 命令卡实时输出 | 未迁移 · `ui-style.md` |
| 72 | 调音引擎 | `todo.md` §待办 1-5 |
| 73 | 命令实时控制与后台运行 | `code-standards.md` · `ui-style.md` |
| 74 | 结果卡凭据告警 | `forbidden.md` §禁用 6 · `todo.md` M2-8 |
| 75 | `agent.exe` 没了 ⇒ 卡片收尾 | 已闭环，见 `archive/agent-feature-backlog-history.md` |
| 76 | 聊天流窗口化渲染 | 已闭环，见 history 归档 |
| 77 | `pruneContext()` 分组别名 | 已闭环，见 history 归档 |
| 78 | 插件子进程随退出终结 | `forbidden.md` §禁用 1 · `code-standards.md` §规范 5 |
| 79 | 待办清单随会话落盘 | 已闭环，见 history 归档 |
| 80 | 审批卡 / 计划卡停靠 | 已闭环，见 history 归档 |
| 81 | 完成提示音 | 已闭环，见 history 归档 |
| 82 | BOM 复查 | `testing.md` §测试 3 |
| 83 | 插件「附属窗」 | 未迁移 |
| 84 | MCP elicitation / roots | 已闭环，见 history 归档 |
| 85 | 多代理通信 | 未迁移（契约） |
| 86 | 代码变更审阅 | 已闭环，见 history 归档 |
| 87 | 本地用量账覆盖未完成回合 | `todo.md` §待办 1-2 |
| 88 | 输出 token 是第一杠杆 | 已闭环，见 history 归档 |
| 89 | 后台活动可观测性 | `logging.md` §日志 5 |
| 90 | 预算只当节流阀 | `todo.md` §待办 4 |
| 91 | pixi `unsafe-eval` | `code-standards.md` §规范 5 |
| 92 | reqwest 回环 `.no_proxy()` | `code-standards.md` §规范 5 |
| 93 | 本地模型决策形态 | `todo.md` §待办 3-6 |
| 94 | 「测试连通」按钮 | `todo.md` §待办 3-6 |
| 95 | http sidecar 就绪判定 | `code-standards.md` §规范 5 |

## 五、新旧映射表（其余旧文档）

| 旧文档 | 现在在哪 |
|---|---|
| `agent-ui-spec.md`（对话区规范 / 运行方式 / 视觉 / 复核清单） | → `ui-style.md` §界面 1–10 · `forbidden.md` §禁用 2；协议字段登记 → archive |
| `icon-style.md`（线性 SVG 参数、残影、emoji 禁令） | → `ui-style.md` §界面 4 / 5 |
| `code-rules.md` | 预检清单 #1–#77 → `testing.md` §测试 4；各章规则 → `code-standards.md` §规范 4–8 · `forbidden.md`；i18n 与提示精简（§13）→ `ui-style.md` §界面 10 |
| `architecture-rendering.md`（渲染层 ADR） | 结论（保留 WebView2 / 不启用 `--disable-gpu` 等）→ `forbidden.md` §禁用 4；实测数据 → archive |
| `agent-implementation.md`（agent 实现现状 / 本地扩展格式） | 工具现状 → archive（`ai-spec.md` §20.1 / backlog 工具面）；扩展格式 → archive §5 |
| `project-layout.md`（目录地图） | → `code-standards.md` §规范 1（分类与硬约束） |

## 六、无法对号入座（已废弃 / 未迁移）的旧章节清单

**已废弃（内容作废或被取代，照旧文档做会错）**：
- `ai-spec.md` §3.7 Trae 参考规范（参照点，实施完毕后不再是规范）；
- `ai-spec.md` §13.2 会话文件清理服务（设想，明确不做；便携式 `temp\` 整目录即清理单位）；
- `ai-spec.md` §17 AI 文本去痕迹 Humanizer（2026-09-30 整体移除）；
- `ai-spec.md` §13「参考设计」的 `core/...` 落点写法（`core/` 是 gitignore 的旧 CLI 参考源码，不在仓库；一律作废）；
- `agent-feature-backlog.md` 旧编号 `backlog §0–§8.x`（含 §2.1–§2.3 子系统组、§3 协议接口层、§5 空转 UI、§8.1–§8.6）——对应条目**全部完成并从清单删除**；
- `architecture-rendering.md` §3 的 B / C 方案（原生主层方案，已否决）；
- `project-layout.md`「内置工具放 `app/src-tauri/tools/`」（2026-10-06 已废弃，改 `agent-templates/tools/`）。

**未迁移（仍有效，但 6 份现行文档未承载，以 archive 原文件为准）**：
- `ai-spec.md` §3.5（agent 协议契约：stream-json 字段与语义、用量口径、压缩、安全分析、各工具的条件注册与事件流）——这是**协议唯一契约**；
- `ai-spec.md` §12（Agent Plan 模式规范，写给模型看的计划写法）；
- `ai-spec.md` §11 中标注「未迁移」的规则（多为协议契约 / 实现细节：15 / 22 / 25 / 28 / 30 / 31 / 37 / 39 / 40 / 54 / 55 / 56 / 57 / 59 / 60 / 64 / 65 / 66 / 69 / 70 / 71 / 83 / 85）；
- `agent-feature-backlog.md` §4.1 长期规划全文（方向性，未排期）、§5 边界声明（工具面核对口径）。

## 七、声明：源码里对旧编号的引用怎么办

> **源码（`app/src`、`core-agent/src`、`scripts/*`）的注释里大量引用旧文档的编号与章节**，例如「见 `ai-spec` §11 规则 44」「预检 #39 ④」「见 ㊹」「backlog §8.4」。
>
> **这些引用仍然指向 `docs/archive/` 下的原文件（旧文件全部整体移动、不删不改名）。** 本文档只做「旧编号 → 新文档」的对照，**不要求逐处回改源码注释**——顺着旧编号到 archive 就能找到原文。新增注释请优先引用本套 6 份文档的新编号（`§规范 N` / `§禁用 N` / `§测试 N` / `§日志 N` / `§待办 N` / `§界面 N`）。

## 八、整合记录（2026-10-11）

- **新建 6 份现行文档**：`code-standards.md`、`forbidden.md`、`testing.md`、`logging.md`、`todo.md`、`ui-style.md`。
- **旧文档去向**：原 8 份文档（`ai-spec.md` / `agent-feature-backlog.md` / `code-rules.md` / `agent-ui-spec.md` / `architecture-rendering.md` / `agent-implementation.md` / `icon-style.md` / `project-layout.md`）**整体移动**到 `docs/archive/`（不删不改名，源码旧编号引用继续可解析）；另新建 `docs/archive/agent-feature-backlog-history.md`（已完成条目归档）。
- **待办拆分**：`agent-feature-backlog.md` 的已完成 / 已闭环条目进 `archive/agent-feature-backlog-history.md`，未完成 + 未验证条目进 `todo.md`（含其调研结论）。
- **纪律**：**以后新规则一律写进这 6 份现行文档，不要回写 `docs/archive/`**（archive 只作历史取证）。`docs/parse-minidump.ps1` 属脚本，**未动**。

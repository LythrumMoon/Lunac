# 日志与诊断规则

> **本文档是什么**：Lunac 的**落盘日志规范**——日志位置、分级、写什么不写什么、脱敏纪律、诊断打点与排查套路。release 是 GUI 子系统、没有控制台、前端也没有 DevTools，**日志是出问题时唯一可查的东西**。
>
> **取代了哪些旧文档的哪些章节**：`ai-spec.md` §11 规则 20（落盘日志）、规则 43（日志分级与降噪）、规则 89（后台活动可观测性）、规则 2（凭据末 4 位 + 来源）、规则 35 / 36（dev 快照遮蔽与 WebView2 profile 留痕）。
>
> **历史细节见 `docs/archive/ai-spec.md` §11。** 提示文案纪律（原始错误进日志、不进界面）见 [`forbidden.md`](./forbidden.md) §禁用 6。

---

## §日志 1 位置与格式

- 两个进程各自落盘到 **`<exe 根>\temp\logs\`**：
  - `agent-YYYY-MM-DD.log`（`core-agent/src/log.rs`）
  - `lunac-YYYY-MM-DD.log`（`app/src-tauri/src/log.rs`）
- 日期为 **UTC**、跨天自动换文件；启动时清理 **7 天前**的 `*.log`（`log::purge_old`，`KEEP_DAYS=7`）。⚠️ 日志时间是 UTC，与本地时间差 8 小时（**已知取舍**）。
- **不引依赖**：不用 `log` / `env_logger` / `chrono`，UTC 时间戳是手写换算（`civil_from_days`）。
- **开关**：
  - `LUNAC_LOG=off` 关闭；
  - `LUNAC_LOG_LEVEL=error|warn|info|debug`（默认 `info`，**`debug` 才记工具输出全文**）；
  - `LUNAC_LOG_DIR` 由宿主注入给 agent，保证两进程写同一目录；agent 独立运行（烟测）时回退 `<agent.exe 目录>\temp\logs`。
- **禁止写 stdout**：agent 的 stdout 是 stream-json 协议流（宿主只转发以 `{` 开头的行），日志只能走文件，否则污染协议。

## §日志 2 写什么（两侧各自的必记项）

- **agent 侧**：启动 / 退出；`ready`（工具清单 / 审批档 / 技能与 MCP 数量）；`cfg`（端点与模型，token 只记 set/empty）；**每次工具调用**（`run_tool` 是唯一入口：名称 + 参数摘要 + 成功或 `FAILED` 文案 + 耗时，覆盖内置 / Skill / MCP）；`run_shell` 的**退出码 / 是否超时 / 输出规模 / stderr 原文**；panic。
- **宿主侧**：启动与退出；`stop_cli`；**agent 的每一行 stderr 原样落盘**（agent 自身没机会写日志时的兜底）；前端经 `log_frontend` 命令上报的未捕获 JS 错误（`window.onerror` / `unhandledrejection`，按内容去重）。
- **明令要求保留、任何时候都不得降级的日志行**（散落在多条规则里）：`请求前缀 #n`（含 system / tools / history 三个前缀哈希与字数）、`摘要压缩：…`、`上下文压缩：…`、`api retry`、`tool X ok` / `tool X FAILED`、`输出超预算（… 全文已落盘）`、`只读工具并行批 N 条`、WebView2 profile 与 browser args 两条、`AI 配置来源` / `agent 凭据`（只留 key_tail）、`auto_start`、`[frontend]`、`shell[...] exit=`、`文件索引已重建`、`launch_elevated`。
- **保留在 stderr 不得顺手删的低频项**：`忽略输入类型`（判据：正在跑的 agent.exe 是旧的）、`忽略非法 JSON 输入行`、`MCP 桥未接通`、`用户输入过长`、`端点不接受 thinking`、`端点回报上下文超限`、`工具轮次达上限`、`用户要求中断本轮`、启动横幅。

## §日志 3 不写什么 / 怎么降噪

- **只降噪，不删证据**：目标是**单位行数的信息密度**，不是行数本身——「删掉能证明结论的那一行」永远是错的；要删的是**同一事实的第二、第三遍转录**。
- **可降级的**（`eprintln` → `log::debug`，或直接删）：
  - `等待审批 {tool}` 由 `eprintln` 改 `log::debug`（它会被宿主以 **warn 级**镜像落盘，一行普通进度被记成警告，曾占 dev 日志行数三分之一）；审批**结果**（超时 / 拒绝）仍 warn / info。
  - 纯重复的直接删：`[agent] 执行工具 {name}` 与 `run_tool()` 的记录重复。
  - 同文本双写合并：同一件事不再在两个日志文件各占一行。
- **`eprintln!` 在 release 下等于没有**：宿主是 `windows_subsystem = "windows"` 的 GUI 进程，stderr 无人接收。**凡是要靠它事后取证的诊断，必须同时写一份到 `log.rs`。**
- **不得把原始错误结构贴进界面**——状态码 / URL / JSON 响应体 / 异常类名进日志，界面只说「哪一步没成 + 用户能做什么」。**第三方拒绝必须落盘**（如 `log_api_reject()`：`{动作} 被拒（状态码）{端点路径} → {响应体前 200 字}`），否则「操作被拒绝」只能靠猜。

## §日志 4 脱敏（硬要求）

- **任何进日志的字符串都要过 `mask_secrets()`**：`sk-` 裸 key、`Bearer <token>`、`api_key` / `search_key` / `token` 等键值对。日志会被用户贴出来求助，凭据不能跟着出门。
- **凭据只记「末 4 位 + 来源」**：启动与保存时各记一行 `AI 配置来源=config\ai.json | .env … key_tail=d423`，喂给 agent 前再记一行 `agent 凭据: endpoint=… model=… key_tail=…`。`log::key_tail()` 是唯一实现（空 → `(empty)`、不足 5 位 → `****`）；**标签必须写成 `key_tail=`**（写成 `key=` / `api_key=` 会被 `mask_secrets` 整体打成 `***`）。全程只出现末 4 位。

## §日志 5 诊断打点纪律

- **打点要能回答它自己提出的那个问题**：跨边界（提问 / 会话 / 进程）的判据，状态就必须跨边界存活，并把判定收口成**可单测的纯函数**。
- **日志行要带「第几次提问 / 本问第几步」**（`问#N` + `#n`），离线看日志不必靠时间戳猜边界。
- **能拿线上报文当证据就别只信本侧变量**（如假端点抓请求体、逐条比对 `messages`）。
- **给「正常但看起来像故障」的形状起个名字**（如 `history=1条` = 新进程签名，此时 `read` 落到 `system + tools` 属正常）。
- **机器读的埋点键值用 ASCII**（`sigma=… delta=… net=…`）；中文散文可以并存。
- **进程可定位**：`run_shell` 在 `spawn()` 成功后立刻记 `shell[..] spawn pid=`；转后台时把 `pid` 放进 `background_started` 事件，前端那行显示 `PID xxxxx`（PID 是语言无关标识符，不进 i18n）。
- **热键模式要落盘**：走内核注册还是全局钩子（`backend=register (no hook)` / `backend=LL-hook (fallback)`）、钩子安装失败、看门狗重装、热键配置回退——这是排查「游戏卡顿 / 热键失效」的第一个事实。
- **同一个 Win32 符号在一个 crate 里只能有一种声明**（多模块要用就抽共享模块，既消警告也消掉两份迟早漂移的 FFI 声明）。

## §日志 6 排查套路（按此顺序，别凭截图猜）

1. **对时间戳**：判「是不是在跑旧二进制」——最后一条 `=== agent start pid=…` 是否晚于被改文件的 mtime；日志里「忽略输入类型: Some(...)」= agent.exe 是旧的。
2. **看缓存与上下文**：`请求前缀 #n` 的公共前缀 `N/M条`（出现 `N < M` 即本侧就地改写）、`请求用量` 行的 `read`（是否塌到 `system + tools` 量级）。
3. **看 `.mcp.json` / `config\mcp.json` 信任门**：`项目 MCP 配置：N 台待过信任门 … fp=<指纹>` 与 `已在信任记录里`——两者都没有 = 配置压根没被读到 / 读的是旧配置；判据是 `agent start pid` 必须晚于配置文件 mtime。
4. **看审批与安全**：审批**结果**（超时 / 拒绝）是 warn / info 级；hook 的 `block` / `error` 走 `system/hook_note`，前端会可见地列出。
5. **看 WebView2**：`WEBVIEW2_USER_DATA_FOLDER` 是否被外部预设（dev / release 共用 profile 会互相污染）；A/B 旗标复启用 `Get-CimInstance Win32_Process | ? { $_.Name -like '*webview2*' }` 读 `CommandLine`（**必须在普通 PowerShell 里跑**，Trae 沙箱会拦 `D:\Lunac\temp\*` 写入导致假数据）。
6. **看「谁在动」**：这台机器上 `lunac.exe` 自身通常不是占用元凶——WebView2 渲染 / GPU 跑在另外的 `msedgewebview2.exe` 里，agent 跑的命令常是 `agent.exe → cmd.exe → 真正干活的程序` 的孙进程；按父进程做传递闭包看。

---

> **历史细节见 `docs/archive/ai-spec.md` §11 规则 20 / 43 / 89。**

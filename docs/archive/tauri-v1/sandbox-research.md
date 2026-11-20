# 沙箱调研与「白箱边界」方案（2026-11-01）

> **本文是什么**：`docs/todo.md` §待办 3-2「U3 真沙箱调研」的**结论文档**。回答三件事：
> ① Windows 上真正能强制执行的隔离手段有哪些、各自代价；② 为什么**不把 Lunac 说成沙箱**；
> ③ 如果目标是「白箱」，应该先做什么、后做什么、明确不做什么。
>
> **本文不是什么**：不是「运行方式三档」（手动 / 白名单 / 自动）的说明 —— 那三档是**策略级**
> 审批口径，与本文的**进程边界**不是一回事，**不得混称**（见 [`forbidden.md`](./forbidden.md) §禁用 1）。
>
> **状态**：调研完成，**未施工**。W0 是否开工由用户拍板；MXC 试点排在其后。

---

## 一、结论（先看这五条）

1. **不做「黑箱沙箱」的产品承诺**。今天 Lunac 的边界是**策略级**（审批 / 工作区锁 / 静态分析 /
   计划模式 / 脚本运行时限额），没有 OS 级隔离。对外只能写「策略边界 + 可见审计」，不能写
   「沙箱 / 隔离环境 / 受控执行」。
2. **「白箱」是产品差异点，不是更弱的沙箱**：白箱 = 边界**可解释、可预演、可审计、可撤销、可关闭**。
   它不替代强制隔离，但即使以后上了 OS 级容器，用户仍然需要知道「刚才为什么被拦、怎么放行」。
3. **近期（不引任何新依赖）先做 W0**：统一决策账本 + 原因码 + 预演 + 一键导出。这是纯产品层
   改动，能把现有散落的判定收成一条可核对的证据链。
4. **中期用 Microsoft Execution Containers（MXC）的 ProcessContainer 做试点**，先只包
   `Cmd` / `PowerShell` 子进程，且**从 `learningModeLogging`（拒绝并记录）起步** —— 它的
   `denials.json` / `denials.verbose.json` 正好就是白箱要的「拒绝证据」。跑顺了再谈 enforcement。
5. **明确不做**：AppContainer 铺全量插件（会打断本地模型 GPU / 插件窗口 / 回环）、WDAC、
   Windows Sandbox、自研驱动 / 文件虚拟化。理由见 §八。

---

## 二、术语：沙箱、白箱、以及我们现在的边界

| 词 | 本文的定义 | 判据 |
|---|---|---|
| **沙箱（本文）** | 由 OS 内核强制执行、**不依赖被约束方配合**的边界 | 即使模型 / 插件 / 脚本「想越界」也做不到 |
| **白箱** | 边界本身对用户**可读**：每条决策有原因码与证据，能预演、能导出、能撤销 | 用户能回答「为什么被拦 / 怎么放行 / 改了什么」 |
| **策略边界（现状）** | 审批 + 工作区锁 + 静态分析 + 计划模式 + 运行时限额 | 能挡「没被批准的动作」，挡不住「被批准后命令自己干的事」 |

两条纪律（已写进 [`forbidden.md`](./forbidden.md) §禁用 1）：

- **不得自称「沙箱」**。策略边界被包装成沙箱，会让用户误判「有兜底」。
- **白箱不等于安全**。可解释只解决「看得见」，不解决「拦得住」；两者必须分开宣传。

---

## 三、今天已经有什么（事实盘点）

| 能力 | 落点 | 强制者 | 白箱程度 | 缺口 |
|---|---|---|---|---|
| 工具审批（允许 / 拒绝 / 始终允许） | `app/src/main.ts` 的 `classifyRequest()`（`CMD_BLACKLIST` 在 `:9134`、判定在 `:9248`） | 前端 + agent 阻塞等待 | 中：危险命令有中文标签；没有统一原因码 | 决策不落账本，事后无法批量核对 |
| 危险命令强制人工 | `core-agent/src/bash_safety.rs`（`dangerous` / `opaque` / `readonly`，见 `:47-51`） | agent 静态分析 + 前端二道网 | 中：标签可读 | 规则命中没有稳定 ID，「为什么是危险」不可追溯 |
| 工作区锁 / 受保护目录 | `core-agent/src/tools.rs` 的 `write_blocked`（`:324`）、`resolve_write_path`（`:1891`）、越界错误 `:1799` | agent | 中：拒绝文案带路径 | 只有词法比较；`Cmd` / `PowerShell` 不解析（`docs/todo.md` 已如实记） |
| 计划模式写闸 | `write_blocked` + 计划相位（`core-agent/src/tools.rs`） | agent | 中 | 与审批卡是两套提示，未合并成一条链 |
| 脚本运行时限额（QuickJS） | `app/src-tauri/src/plugin_engine.rs`（`DEFAULT_TIMEOUT_MS` / `MAX_CODE_BYTES` / `MEMORY_LIMIT_BYTES`，`:27-38`） | 进程内运行时 | 低：只返回 `E_TIMEOUT` 等字符串 | 没有把「哪个插件、哪段代码、哪条限额」记进统一账本 |
| sidecar 资源闸（Job Object） | `app/src-tauri/src/child_job.rs`、`plugin_sidecar.rs:377-404` | 内核（内存 / 进程数 / 杀树） | 低：只有触顶行为 | 资源闸不是隔离；没有「谁触顶」的审计行 |
| 文件快照回退 | `app/src-tauri/src/snapshots.rs` | 宿主（事后） | 中：可撤销 | 只覆盖写文件；命令的副作用不可撤销 |

**一句话**：今天有「拦截点」，缺「证据链」。白箱要补的正是后者。

---

## 四、Windows 候选机制对比

> 列里只写**能验证的事实**；带【推论】的会在 §八 再标一次，不能当结论用。

| 机制 | 强制层 | 能限制什么 | 不能限制什么 | 对 Lunac 的代价 | 白箱产物 | 结论 |
|---|---|---|---|---|---|---|
| **策略边界（现状）** | 前端 + agent | 需审批的动作；工作区内写 | 被批准后的命令行为、网络、读取 | 0 | 审批卡 + 日志（分散） | 保留，作为默认 |
| **Job Object** | 内核 | 进程内存 / 进程数 / 随句柄关闭杀树 | 文件、网络、读、UI | 低（已有 `child_job`） | 限额值 + 触顶事件 | 保留，仅作资源闸 |
| **低完整性级别（Low IL）** | 内核 MIC | 默认 **NO_WRITE_UP**：低完整性进程写不了中完整性对象 | 读（默认允许）；网络；执行 | 中：要造低 IL 令牌 + 启动路径 | Access denied（难解释） | 不单独用 |
| **受限令牌（Restricted Token）** | 内核 | 删权限 + 限制 SID；`WRITE_RESTRICTED` 时写访问要过两道检查 | 细粒度网络 / UI；需要 broker 才有可用性 | 中高：目标目录要先铺 ACL | 双检查拒绝（需自己解释） | 不单独用 |
| **AppContainer / LPAC** | 内核 | 文件 / 注册表 / 能力的默认拒绝（capability SID） | 设备 / 桌面交互要额外能力；回环需豁免[^10] | 高：会把插件窗口、本地模型 sidecar、`127.0.0.1` 服务全打断 | 能力拒绝（可枚举） | 不铺全量 |
| **MXC ProcessContainer** | 内核（AppContainer 底座 + 策略） | 文件（只读 / 读写 / 拒绝）、网络出站、UI / 剪贴板 | 不支持的平台；部分能力需实测 | 中高：引入 `mxc-sdk`，只支持 Win11 | **denials.json + 学习模式 + 导出** | **推荐试点** |
| **MXC Session / WSLc / MicroVM** | 会话 / Linux / 虚拟化 | 更强隔离、独立桌面 | 资源开销；Session / WSLc 无同等拒绝采集 | 高 | 弱（仅 ProcessContainer 有学习模式） | 后置 |
| **WDAC（App Control）** | 内核代码完整性 | 只决定「这个二进制 / 脚本能不能跑」 | 运行时文件 / 网络 / 读 | 高：签名策略 + 管理员；误杀插件 | CodeIntegrity 事件日志 | 不做 |
| **Windows Sandbox** | Hyper-V | 整机级隔离（独立桌面） | 不能嵌入现有应用；Pro / Enterprise | 很高 | 无应用内账本 | 不做 |

引用与细节：AppContainer 的 ACL / capability 模型见 Microsoft Learn[^1]；Low IL 默认只拦写
（`SYSTEM_MANDATORY_LABEL_NO_WRITE_UP`）见[^2]；Restricted Token 的「两次访问检查」与
`WRITE_RESTRICTED` 见[^3]；MXC 见 §五。

---

## 五、MXC：目前最接近「白箱」的 OS 级载体

### 5.1 事实

- Microsoft 于 **2026-10-07** 宣布 MXC **GA**，定位是「策略驱动的执行层」，用于模型输出 /
  插件 / 工具 / 整个 agent；策略在 workload 之外，agent 不能给自己加权限[^4]。
- 仓库 [`microsoft/mxc`](https://github.com/microsoft/mxc) **MIT**；提供 **Rust / .NET / Node SDK**
  （Rust crate `mxc-sdk`）与独立执行器 `wxc-exec.exe`；策略是**版本化 JSON**。
- Windows 后端：默认 `processcontainer`；另有 `windows_sandbox` / `wslc` / `microvm` /
  `hyperlight` / `isolation_session`（其中 `windows_sandbox` / `microvm` / `hyperlight` 在
  README 里标注 experimental；`wslc` / `isolation_session` 需特定 Windows 版本）。
- ProcessContainer 的 Windows 版本下限（官方表，**以运行时探测为准**）：
  24H2 `26100.9278`、25H2 `26200.9278`、26H2 `26300.9550`、26H1 `28000.2804`[^5]。
- 策略覆盖面：**文件系统**（只读 / 读写 / 拒绝路径）、**网络**（出站 / 代理 / 回环）、
  **UI**（剪贴板 / 显示 / GUI）、**进程**（命令行 / 工作目录 / 环境）。

### 5.2 为什么说它「白箱」

MXC 提供三种运行模式[^4]：

| 模式 | 未授权访问 | 活动报告 | 用途 |
|---|---|---|---|
| **Enforcement** | 拦截 | 无 | 生产策略 |
| **Learning** | 拦截**并记录** | 有 | 诊断失败、反推最小权限策略 |
| **Permissive** | 放行**并记录** | 有 | 只观察不强制（审计） |

对应用自己的「用户可配置」场景，官方推荐 `captureDenials`（`mode: "block"` = 拒绝并记录；
`mode: "allow"` = 放行并记录），并明确 **`block` 模式仍保持 deny-by-default**，workload 行为与
生产一致[^6]。这正是白箱需要的东西：**不放宽安全，也能拿到证据**。

产物（`--audit` / 学习模式）包括：`denials.json`、`denials.verbose.json`、`trace.etl`、
`Adjusted_*.json`（按拒绝项自动调整后的策略草案）[^6]。`denials.json` 的结构：

```json
{
  "denials": [
    {
      "resource": "C:\\Users\\me\\secret.txt",
      "resourceType": "file",
      "accessType": "read",
      "pid": 1234,
      "filetime": "132847890123456789"
    }
  ],
  "summary": { "exitCode": 0, "totalDenials": 1, "deniedResourcesTruncated": false }
}
```

- `resourceType` ∈ `file` / `ui` / `network` / `capability` / `other`；
  `accessType` ∈ `read` / `write` / `execute` / `unknown`。
- 网络拒绝会带原因与建议：`directDefaultDeny`（默认拒绝，建议 `addEgressAllow`）、
  `authoredExplicitDeny`（显式拒绝，加 allow 无效）、`allowExclusion`、`proxyContainment`、
  `appIsolationMissingCapability`（建议补 capability）[^6]。
- 去重按 `(resource, accessType)`；最多保留 10,000 条唯一拒绝、处理 1,000,000 条 ETW 事件[^6]。

这两点让它比「自己写日志」强：**拒绝是 OS 判的，记录也是 OS 给的**，不依赖 agent 配合。

### 5.3 必须如实记的限制

- **只支持 Windows 11**（且要满足上表的补丁级）；Win10 用户没有这条路，只能回落到现状。
- ProcessContainer 的底座是 AppContainer：**设备 / GPU / 桌面交互 / 回环**都要额外授权。
  本地模型 sidecar（Vulkan）、插件 iframe 窗口、`127.0.0.1:8788/8789` 这类用法会受影响；
  哪些真跑不通**需要实测**，本文不预设结论（与 `docs/todo.md` 的【推论】口径一致）。
- Learning Mode 依赖 Windows 的 PSEC / Learning Mode 导出；不满足时 MXC 会以
  `backend_unavailable` **在建容器前失败**，不会静默降级[^5][^6]。我们的接入必须把这条
  失败路径做成「回落现状 + 一行可读提示」。
- `permissiveLearningMode`（allow-and-record）**会关闭 deny-by-default**，属于安全敏感模式：
  只能用于开发者 / 审计场景，产品默认不得开启；SDK 会通过 `warnings()` 显式返回安全警告[^6]。
- Session container / WSLc **没有**同等的过程级拒绝采集：官方的学习模式只针对 **Windows
  ProcessContainer**（Windows-only，且依赖 PSEC / Learning Mode 导出）[^5][^6]。想要白箱证据，
  目前只有 ProcessContainer 这一条路。
- Rust SDK 会把 runtime / backend **编进应用**，构建时间与体积都会涨；官方要求 Rust **1.93+**，
  本项目当前工具链满足。
- MXC 很新（2026-10 GA）。本项目的口径是「纵深防御的一层」，**不把它当唯一安全承诺**。

---

## 六、白箱边界的设计（Lunac 自己的五件套）

> 目标不是「再加一个开关」，而是让**每一次边界判定都留下一条可核对的记录**。

### 6.1 决策账本（W0 的核心）

- 落点：`<exe 根>\ModuleData\security\decisions-YYYYMMDD.ndjson`（按天分片；**不进 `chat.db`**）。
- 每行一条 JSON，字段固定（新增字段只许追加）：

```json
{
  "v": 1,
  "ts": "2026-11-01T05:12:33Z",
  "session": "…",
  "actor": "agent | plugin | pet | cron",
  "tool": "Cmd",
  "target": "powershell -c …",
  "decision": "allow | ask | deny | auto",
  "reasonCode": "DANGER_RULE",
  "reasonText": "递归强制删除",
  "policySource": "bash_safety.rs#danger.recurse_delete",
  "scope": "workspace:D:\\proj",
  "evidence": { "matched": "rm -rf", "analysis": "dangerous" },
  "reversible": false
}
```

- **脱敏**：`target` / `evidence` 进盘前必须过 `mask_secrets()`（[`forbidden.md`](./forbidden.md) §禁用 6）。
- **上限**：单文件 ≤ 32 MiB、目录最多 14 天、总计 ≤ 200 MiB；超限先删最旧的分片。
- **导出**：一键打包 `ledger + policy.json + 版本 / OS build`，**不含** API Key / Cookie。

### 6.2 原因码（唯一枚举）

| 码 | 含义 | 现状来源 |
|---|---|---|
| `AUTO_READONLY` | 可证只读，自动放行 | `bash_safety.rs` 的 `readonly` |
| `WHITELIST_TOOL` / `WHITELIST_CMD` | 命中用户白名单 | `lunac-approve-whitelist` |
| `ASK_DEFAULT` | 默认要问 | 前端 `classifyRequest()` |
| `DANGER_RULE` | 命中危险规则（永不给始终允许） | `bash_safety.rs` 的 `dangerous` |
| `OPAQUE_COMMAND` | 判不出来，fail-closed | 同上 `opaque` |
| `SECRET_IN_PAYLOAD` | 写入内容里扫出疑似凭据 | `main.ts` 的 secrets 扫描 |
| `PLAN_MODE_BLOCK` | 计划模式下的写类拒绝 | `tools.rs` 的 `write_blocked` |
| `WORKSPACE_LOCK` | 越出工作区 | `tools.rs:1799` |
| `PROTECTED_DIR` | 受保护目录（`config\` / `Modules\` 等） | `tools.rs` |
| `USER_ALLOW` / `USER_DENY` / `USER_ALWAYS` | 用户点的那一下 | 审批卡 |
| `ENGINE_TIMEOUT` / `ENGINE_OOM` | QuickJS 限额 | `plugin_engine.rs` |
| `MXC_FILE_DENY` / `MXC_NET_DENY` / `MXC_CAP_DENY` / `MXC_UI_DENY` | MXC 拒绝 | `denials.json`（W1 起） |
| `BACKEND_UNAVAILABLE` | MXC 不可用，回落现状 | W1 起 |

原因码是**稳定契约**：UI、账本、导出、测试都只认它，**不许在调用点拼中文字符串**。

### 6.3 预演（dry-run）

- 入口：设置 → 安全 → 「预演一次」 / 审批卡上的「先看会怎么判」。
- 行为：只跑**判定流水线**（工作区 / 静态分析 / 白名单 / 计划相位），**不执行工具**；
  输出「会 allow / ask / deny + 原因码 + 命中规则」。
- 这不是 MXC 的 `--audit`（那个会**放行并记录**真实访问），两者不要混称。

### 6.4 取证与导出

- 审批卡上增加一行「为什么问我」：`reasonCode + reasonText + policySource`；
  拒绝后给「怎么放行」：加白名单 / 换工作区 / 改运行方式（危险命令**不提供**放宽）。
- 设置 → 安全 → 「最近决策」（最近 200 条）+ 导出按钮。
- 与 MXC 对接后，`denials.json` 的每条拒绝都映射成一个原因码写进同一本账。

### 6.5 撤销

- 文件类：复用 `snapshots.rs`（已有按会话的 `frames\*.json`）。
- 命令类：**如实标 `reversible: false`**，不承诺回滚；需要回滚的场景引导用户用版本控制。
- 不做「全局一键回滚」（命令副作用无法完整捕获，承诺了就是骗人）。

---

## 七、分阶段落地

| 阶段 | 内容 | 改动面 | 验收判据 | 风险 / 回滚 |
|---|---|---|---|---|
| **W0 白箱账本**（2026-11-01 已落地） | 决策账本 + 原因码 + 预演 + 导出 + 审批卡「为什么问我」 | 宿主 `app/src-tauri/src/security_ledger.rs`（3 条命令）；前端 `app/src/security-ledger.ts` + `main.ts` 的 `describeRequest / logBoundary`；入口 = 聊天窗「更多设置 → 安全」（在「其他」之前） | 随机抽 20 次工具调用，账本 20/20 有对应行；导出不含凭据；重启后仍在；关开关后账本为零 | 纯增量；出问题直接关开关 |
| **W1 MXC 学习模式** | 只包 `Cmd` / `PowerShell` 子进程；`learningModeLogging`（拒绝并记录）；策略 = 工作区读写 + 系统只读 + 其余拒绝 + 网络默认拒绝 | `app/src-tauri` 引入 `mxc-sdk`；工具子进程启动路径；`denials.json` → 账本 | 3 个真实任务各跑一遍：每次失败都能在 `denials.json` 找到 `resourceType` / `accessType`；关闭开关后与今天行为一致；Win10 上明确显示「不可用，回落到策略边界」 | 需要 Win11 补丁级；失败即回落；不改变默认行为 |
| **W2 强制执行** | 对 `Cmd` / `PowerShell` 打开 enforcement；按回合 / 工作区生成策略 | 同上 + 策略生成器 + UI 能力边界说明 | 越界写在 OS 层被拒（不是靠提示词）；工作区内构建 / 测试通过；被拒时 UI 给原因码与放行建议 | 误杀风险高：先只对「自动档 + 非交互」开，可一键退回 W1 |
| **W3 更强隔离** | Session container（长任务）/ WSLc（Linux 工具链）/ MicroVM（实验） | 大 | 单独评估 | 暂不排期；Session / WSLc 没有 denial 采集，白箱会退化 |

**建议顺序的理由**：W0 不依赖 OS 版本、不改变行为，先把「证据链」建起来；W1 用它接住 MXC 的
拒绝记录；W2 才谈强制。否则一上来做隔离，用户看到的仍然是一个「不知道为什么被拦」的黑箱。

---

## 八、明确不做（以及为什么）

1. **不把 AppContainer 铺成插件底座**：项目此前已裁定（`docs/todo.md` §待办 3-10 E.7 第 7 条）
   —— 会拦设备 / 网络，本地模型与插件窗口先坏；"CUDA / Vulkan 走不通"目前是【推论】，
   但代价足够高，不值得为了「听起来更安全」去赌。
2. **不做 WDAC**：它只管「二进制能不能加载」，不管运行时文件 / 网络；需要签名策略与管理员，
   还会误杀用户自己构建的工具。与「白箱」目标不匹配。
3. **不做 Windows Sandbox**：Hyper-V、Pro / Enterprise、独立桌面，无法嵌入现有应用，
   也不可能把审批卡 / 账本接进去。
4. **不做自研驱动 / 文件虚拟化（minifilter / ProjFS）**：维护成本与蓝屏风险远超收益；
   真要强隔离，直接用 MXC 的成熟后端。
5. **不把 MXC 的 `--audit` / `permissiveLearningMode` 作为产品默认**：它们会**关闭**
   deny-by-default，只适合开发 / 审计。产品里只用 `captureDenials` 的 `mode: "block"`。
6. **不承诺「无人值守安全」**：即使 W2 落地，也只覆盖被容器包的子进程；宿主自身、已批准的
   网络请求、用户手动执行的命令都不在边界内。文案仍按 [`forbidden.md`](./forbidden.md) §禁用 1。

---

## 九、验收清单（白箱是否成立）

- [ ] 任意一次「自动放行 / 人工确认 / 拒绝」都能在账本里找到**一行**，且含原因码与来源。
- [ ] 审批卡能回答「为什么问我」，拒绝后能回答「怎么放行」。
- [ ] 预演不执行工具，输出与真实判定一致（同一原因码）。
- [ ] 导出包能独立复盘：不含凭据；含版本 / OS build / 策略版本。
- [ ] 关掉白箱开关后，行为与今天**逐字节一致**（日志为零、无额外进程）。
- [ ] （W1 起）MXC 不可用时回落现状，并明确显示 `BACKEND_UNAVAILABLE`。
- [ ] （W1 起）`denials.json` 的每条拒绝都能映射到账本原因码，无「只记录不解释」。

---

## 十、参考

[^1]: Microsoft Learn — [实现 AppContainer](https://learn.microsoft.com/zh-cn/windows/win32/secauthz/implementing-an-appcontainer)（ACL / capability SID 模型，2025-03-13 更新）。
[^2]: Microsoft Learn — [Mandatory Integrity Control](https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control)（默认 `NO_WRITE_UP`；低完整性进程默认仍可读）。
[^3]: Microsoft Learn — [CreateRestrictedToken](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken)（两次访问检查；`WRITE_RESTRICTED` 只约束写）。
[^4]: Windows Developer Blog — [Microsoft Execution Containers: Policy-driven containment for AI agents](https://blogs.windows.com/windowsdeveloper/2026/10/07/microsoft-execution-containers-policy-driven-containment-for-ai-agents/)（2026-10-07，GA；三种模式；容器后端表）。
[^5]: MXC 仓库 — [`docs/backends/process-container/os-version-support.md`](https://github.com/microsoft/mxc/blob/main/docs/backends/process-container/os-version-support.md)（Windows 版本下限；Learning Mode 依赖 PSEC 导出，运行时探测为准）。
[^6]: MXC 仓库 — [`docs/logging-access-denied.md`](https://github.com/microsoft/mxc/blob/main/docs/logging-access-denied.md)（`captureDenials` / `learningModeLogging` / `permissiveLearningMode`；`denials.json` 字段与网络拒绝原因）。
[^7]: MXC 仓库 — [`README.md`](https://github.com/microsoft/mxc/blob/main/README.md)（MIT；Rust / .NET / Node SDK；`wxc-exec.exe`；ProcessContainer 为 Windows 默认后端）。
[^8]: Microsoft Learn — [Windows Sandbox 概述](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-overview)（Hyper-V 隔离、独立桌面、Pro / Enterprise 限定）。
[^9]: Microsoft Learn — [App Control for Business（WDAC）](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/app-control-for-business/appcontrol)（代码完整性策略，需签名 / 管理面）。
[^10]: Microsoft Learn — [排查 UWP 应用连接问题](https://learn.microsoft.com/zh-cn/windows/security/operating-system-security/network-security/windows-firewall/troubleshooting-uwp-firewall)（AppContainer 回环默认需 `CheckNetIsolation LoopbackExempt` 豁免）。

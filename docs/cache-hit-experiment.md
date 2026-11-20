# 三环境对照实验 v2（A / B / C）——本地决策价值

> **状态**：已执行（2026-10-24，A / B / C 各 1 次；结果、结论与优化建议见 §6）。**登记处**：[`todo.md` §待办 3-12](./todo.md)。
> **v2 一句话**：同一任务书、各环境**新开一次对话**，比较 A(Trae) / B(Lunac 本地决策关) / C(Lunac 本地决策开) 的
> **未命中输入 token / 估算成本 / 任务完成度**；缓存命中率**降为辅助指标**。
>
> **v1（2026-10-24）结论已作废**：平台用量 CSV 只有密钥维度，A 走 Trae 自带通道、B/C 共用一把 key，
> 三把 `TEST A/B/C` 密钥合计只有 4 次探活请求；C 组 `LUNAC_LOCAL_DECIDE` 未设、本地模型加载后零推理。
> ⇒ 命中率不可归因，C 组从未发生。v2 只改**流程与指标**，任务书正文（§3）保持逐字节不变。

> **模型 / API 不在本文件记录**：由执行者自行配置，三组使用同一模型（建议 `deepseek-flash`）。
> 各组的端点与计费通道可能不同，这一点只在结论里作「已知偏差」说明，不作为实验变量。

---

## 0. 硬约束（先读，违反则整组作废）

1. **本仓库只读**：`d:\cc\claude-code-cli-master` 允许 `Read` / `Grep` / `Glob`，**禁止** `Write` / `Edit` / 删除 / `git` 写操作。
2. **所有写操作只落 `D:\test\<A|B|C>\`**（含代码产物与取证文件）。
3. **三组任务书逐字节相同**（唯一允许的差异 = 输出目录 `{OUT}` 的替换，见 §3.2）。
4. **每组新开一个对话**（空历史），不在旧会话里续跑。
5. **取不到的指标一律写 `null` / 「无法取证」，不许编造。**
6. **`TEST API.txt` 里的任何内容（尤其凭据）不得写进代码 / README / 日志 / report / 提示词**；只允许运行时读取使用，取证只记「是否调用 + 结果摘要」。
7. **正文必须用本组的 `TEST X` 密钥**（2026-10-24 新增）：
   - B/C 在 Lunac 设置里把 AI 密钥换成 `TEST B` / `TEST C`，并在 agent 启动日志里核对
     `agent 凭据: … key_tail=` —— 只需确认末 4 位与 `TEST X` 一致；
   - A 若平台支持自定义 OpenAI 兼容端点，同样换 `TEST A`；**不支持就如实写「通道不可替换」**，
     A 只参加定性对照（任务完成度 / 子代理能力），**不参加成本与命中率统计**；
   - 禁止复用软件自身配置里的 key 跑正文（v1 的根因）。
8. **C 组必须真的发生本地推理**（2026-10-24 新增）：
   - `LUNAC_LOCAL_DECIDE=1`；
   - 宿主日志出现 `本地模型就绪：…（model=…）` 或前端状态显示「已就绪」；
   - agent 日志出现任一判定行（`检索查询改写` / `记忆候选抽取` / `安全预筛` / `能力路由`）。
   - 三条缺一 ⇒ 整组作废（等价于 B，不能叫 C）。
9. **B 组必须确认本地决策没参与**：`LUNAC_LOCAL_DECIDE` 不设或 `0`，且 agent 日志对上述四类判定行 **0 命中**。
10. **B / C 两项额外开关保持关闭**（否则压缩行为不止「本地决策」一个变量）：
    `LUNAC_LOCAL_CURATE`、`LUNAC_COMPACT_APPEND_ONLY`；`LUNAC_MAX_CONTEXT_TOKENS` 只在强制档同设 `48000`。

---

## 1. 三组定义

| 组 | 环境 | 输出目录 | 正文密钥 | 本地决策 | 其它 |
|---|---|---|---|---|---|
| **A** | Trae | `D:\test\A` | `TEST A`（通道允许时） | 不适用 | 平台默认；人格在 Trae 内置配置里设置；通道不可替换时只做定性对照 |
| **B** | Lunac | `D:\test\B` | `TEST B` | **关** | `LUNAC_LOCAL_DECIDE` 不设或 `0` |
| **C** | Lunac | `D:\test\C` | `TEST C` | **开** | `LUNAC_LOCAL_DECIDE=1` + §0.8 三条门禁 |

**三组都必须有**两份输入（由执行者放进各自目录，内容可不同）：

- `TEST API.txt` —— 本次任务可调用的额外接口说明（agent 运行时读，见 §3.1 的「零、先看工作目录」）；
- 人格提示词 —— **在 Trae / Lunac 各自的产品配置里设置**，三组内容一致；**不写进本文件、也不写进 `taskbook.md`**。

以下三项在 B / C **都必须保持关闭**（否则压缩行为就不止「本地决策」一个变量）：

- `LUNAC_LOCAL_CURATE`（本地策展）
- `LUNAC_COMPACT_APPEND_ONLY`（append-only 全量压缩）
- `LUNAC_MAX_CONTEXT_TOKENS`（自然档不设；强制触发档 B/C 同设 `48000`）

---

## 2. 准备（人工，约 5 分钟）

### 2.1 建目录

```powershell
New-Item -ItemType Directory -Force D:\test\A\report, D:\test\B\report, D:\test\C\report | Out-Null
```

### 2.2 环境变量（只对 B / C）

| 变量 | B | C |
|---|---|---|
| `LUNAC_LOCAL_DECIDE` | 不设 / `0` | `1` |
| `LUNAC_LOCAL_MODEL_URL` | **由宿主自动注入**（装了 `local-model` 插件 + 设置里打开） | 同左，且必须确认已注入 |
| `LUNAC_LOCAL_CURATE` | 不设 | 不设 |
| `LUNAC_COMPACT_APPEND_ONLY` | 不设 | 不设 |

> `LUNAC_LOCAL_MODEL_URL` 不是手工设的：宿主只在「插件已装 + 设置开关打开」时注入。**C 组不满足这一条就等于 B 组**，实验无效。
> 2026-10-24 起宿主会在 sidecar 启动后做**真实探活**：日志出现 `本地模型就绪：…` 才算「模型能用」；
> 只有 `本地决策模型端点 → agent` 一行**不算**（v1 就是只看了这一行）。

### 2.3 前置自检（把证据存进各组的 `report\preflight.txt`）

| 组 | 检查项 | 通过判据 |
|---|---|---|
| A | 是否支持自定义 OpenAI 兼容端点 / 并行子 agent | 手写记录（支持 / 不支持） |
| A/B/C | 目录里有 `TEST API.txt` | 文件存在（不记录内容） |
| A/B/C | **正文用的是本组测试密钥** | Lunac：`agent 凭据` 行 `key_tail=` 与 `TEST X` 末 4 位一致；Trae：面板 / 配置页截图（取不到写「无法取证」） |
| B | agent 日志**没有**本地判定行 | 搜索 `检索查询改写` / `记忆候选抽取` / `安全预筛` / `能力路由`，应 0 命中 |
| C | 端点已注入 | 日志有 `本地决策模型端点 → agent: http://127.0.0.1:…（N 槽）` |
| C | **模型真的就绪** | 宿主日志 `本地模型就绪：…（model=…，N ms）`，或前端「更多设置 → 其他」显示「已就绪」 |
| C | 判定真的在跑 | agent 日志出现上面任一判定行（或 `本地判定预热完成`） |
| B/C | 记录本次 `LUNAC_MAX_CONTEXT_TOKENS` 实际值 | 自然档 = 缺省（未设）；强制档 = `48000` |
| B/C | 本地决策是否启用 | `decide=0`（B）/ `decide=1`（C），写进 preflight |

`report\preflight.txt` 建议模板：

```text
env=A|B|C
key_tail=xxxx          # 只记末 4 位；取不到写 null
channel_replaced=yes|no|unavailable
decide=0|1|n/a
local_model_probe=ready|failed|off|n/a
decision_lines=0
max_context_tokens=unset|48000
parallel_subagents=yes|no|unknown
```

Lunac 的日志位置：
- dev 实例：`app\src-tauri\target\debug\temp\logs\agent-YYYY-MM-DD.log`
- 安装版：`<安装根>\temp\logs\agent-YYYY-MM-DD.log`

### 2.4 生成三份任务书 `taskbook.md`（**不含人格提示词**）

> 人格提示词在 Trae / Lunac 各自的**产品配置**里设置（三组内容一致），**不进入本文件、也不进入 `taskbook.md`** —— 只有这样三份任务书才能靠 sha256 对照。

1. 取 §3 的**母版**（含 `{OUT}` 占位）。
2. 分别把 `{OUT}` 替换成 `D:\test\A` / `D:\test\B` / `D:\test\C`，各自存成 `D:\test\<组>\taskbook.md`。
3. 记录两样东西进 `report\manifest.txt`：
   - 替换前的**母版 sha256**（三组必须相同）；
   - 各组替换后的 `taskbook.md` sha256。

```powershell
Get-FileHash D:\test\A\taskbook.md -Algorithm SHA256
Get-FileHash D:\test\B\taskbook.md -Algorithm SHA256
Get-FileHash D:\test\C\taskbook.md -Algorithm SHA256
```

---

## 3. 任务书（三组完全相同）

### 3.1 母版（整段复制到各组对话；只替换 `{OUT}`）

```text
【本轮任务】
你要在一个工作目录里独立完成一个小型 Rust 库，并留下可复核的证据。严格按下面的顺序做。
（人格由运行环境配置，不在本任务书内 —— 不要重述、也不要修改它。）

零、先看工作目录
1) {OUT} 下有一份 `TEST API.txt`：按它的说明做。若它给出可调用的接口，就做一次最小连通性调用；
   若它只是说明而没有可调用地址，就记「无需调用」。
2) `TEST API.txt` 的内容（尤其凭据）不得写进代码 / README / 日志 / report / 提示词；只在运行时读取使用。
3) 把「是否调用 + 结果摘要（不记凭据、不记 key）」写进 {OUT}\report\test-api.txt。

一、先读规范（只读，不许写仓库）
1) 用只读方式通读下列 6 份文档：
   d:\cc\claude-code-cli-master\docs\README.md
   d:\cc\claude-code-cli-master\docs\code-standards.md
   d:\cc\claude-code-cli-master\docs\forbidden.md
   d:\cc\claude-code-cli-master\docs\logging.md
   d:\cc\claude-code-cli-master\docs\testing.md
   d:\cc\claude-code-cli-master\docs\todo.md（重点看「§待办 3-6 补 P6」两节）
2) 在 {OUT}\report\doc-checklist.md 里，为每份文档写一条「本文档对本次交付的约束」（一行一份，共 6 行）。
3) 读完再开始写代码。

二、任务：实现「压缩指针回注」的纯函数
1) 在 {OUT}\ptrblock\ 新建独立 Rust crate（不依赖 d:\cc\claude-code-cli-master 里的任何 crate，
   也**不引任何外部 crate**：只许 std，保证离线也能 cargo test）。
2) 实现：
   pub struct Elided { pub tool: String, pub input: Vec<(String, String)> }  // input = 已从 tool_use 入参摊平的键值对
   pub fn render_pointers(elided: &[Elided], recent_requests: &[String], budget_chars: usize) -> String
3) 规则（逐条都要实现，不许省略）：
   a. 只列「可重跑」工具：Read / Grep / Glob / Cmd / PowerShell / WebFetch / WebSearch；其余忽略。
   b. 每条渲染成：`- Read src/main.rs (re-runnable)` / `- Grep "protected_reason" in src/ (re-runnable)`。
   c. 去重：同 tool + 同一关键入参只留一条。
   d. 稳定排序：先按「是否出现在 recent_requests（路径/关键词子串）」降序，再按渲染文本字典序。
   e. 首行固定 `[elided pointers]`；总长超 budget_chars 时按上述顺序截断，末行 `... (+N more)`。
   f. 空输入 => 返回空串（不是只剩首行）。
   g. 渲染前先做凭据打码：`sk-...`、`Bearer ...`、`key=value` 的值 -> `***`；打码后的文本才参与长度计算。
   h. 单条渲染文本超过 200 字符时，截到 200 字符并追加 `...`（先逐条截断，再算总预算）。
   i. 纯函数：零网络、零磁盘、零全局状态。
4) 测试：cargo test 至少 10 条，必须覆盖：去重 / 近期命中优先 / 预算截断与 (+N more) / 空输入 / 非可重跑工具被忽略 / CJK 路径或关键词 / 凭据打码 / 单条超长截断。
5) README.md 写清接口签名、每条规则与一个输入输出示例。
6) 自检：cargo test 全绿；cargo clippy 无 error（允许 warn）。

三、并行子任务（必须真正并发发起，至少 3 个）
- ① 输入归一 + 去重；② 排序 + 预算截断 + 渲染；③ 测试与夹具。
- 在 {OUT}\report\subagents.txt 记录：子任务名、发起时间、返回摘要、各自改了哪些文件。
- 如果你的环境不支持并行子 agent：如实写「不支持」，不要串行做完再写成并行。

四、边界
- 禁止向 d:\cc\claude-code-cli-master 写任何东西（只读）。
- 禁止把任何规则写进系统提示词。
- 禁止引入网络依赖。

五、收尾
- 把 {OUT}\ptrblock 下的文件清单 + 每个文件的 sha256 写进 {OUT}\report\manifest.txt（追加在已有内容后）。
- 最后回一段「自检结论」：6 份文档各一条约束是否满足、测试条数、clippy 结果、你用了几个子任务。
```

### 3.2 唯一允许的文本差异

- 只有 `{OUT}` 会被替换成 `D:\test\A` / `D:\test\B` / `D:\test\C`。
- 除此以外**一个字都不许改**（任务正文、标点、换行）。
- 粘贴前先把母版存一份 `D:\test\taskbook-master.md`（含 `{OUT}`），用于核对。

> **任务书刻意不写「必须用 role=coding」**：A 没有这个参数，写进去会破坏三组可比性。
> 角色由执行者在 B/C 里**自然使用**（模型自己选），只进 `subagents.txt` 记录，不作为通过条件。

---

## 4. 上下文压缩：至少 1 次

### 4.1 自然档（首选）

- **判定**：
  - Lunac：`agent-*.log` 出现 `上下文压缩：…`（或前端 `context_compacted`）。
  - Trae：会话里出现压缩 / 上下文截断提示；取不到日志就写「无法取证」。
- **不需要额外设置**：`docs\` 那 6 份合计 **3,139 行 / ~497 KB**（`todo.md` 单份 ~380 KB），只读规范就会吃掉大量 input token。
- **C 组必须先过 §2.3 门禁**：本地决策没真正跑起来的压缩数据同样作废。

### 4.2 强制触发档（自然档没触发时）

- **B、C 各补跑一次**，两组**同设** `LUNAC_MAX_CONTEXT_TOKENS=48000`，并在 `report\compaction.txt` 标 `mode=forced`。
- A 无法设置 ⇒ 如实记「未强制」。
- **强制档与自然档分开统计，不混算。**

### 4.3 必须记录（`report\compaction.txt`）

```text
mode=natural|forced
turn=第几轮触发
before_turn_usage: in=..., cache_read=..., cache_creation=...
after_turn_usage:  in=..., cache_read=..., cache_creation=...
recall_check=压缩后模型复述「二、任务 第 3 条 a-i」是否完整（pass / partial / fail）
evidence=日志行号或截图文件名
```

`recall_check` 的问法（压缩后紧接着问，逐字使用）：

```text
请只复述任务书里「二、任务」第 3 条 a-i 的规则，不要写代码、不要解释。
```

---

## 5. 取证模板（每组 `report\` 下）

### 5.1 `conversation.jsonl`（每轮一行）

```json
{"turn":1,"role":"user","summary":"读 6 份规范","tools":["Read"],"usage":{"in":null,"out":null,"cache_read":null,"cache_creation":null}}
{"turn":2,"role":"assistant","summary":"doc-checklist 6 行","tools":["Write"],"usage":{"in":12034,"out":512,"cache_read":0,"cache_creation":12034}}
```

> 取不到 usage 就写 `null`。**不要估算、不要补零。**

### 5.2 `usage.csv`（v2：本地账是主口径）

```csv
turn,in,out,cache_read,cache_creation
1,12034,512,0,12034
2,...
```

- **B/C 主口径**：`temp\usage\usage-YYYY-MM-DD.jsonl`（逐请求）或 agent 日志里的 `usage` 行 ——
  必须包含 `in` / `out` / `cache_read` / `cache_creation` 四项**原始值**。
- **平台 CSV 只作交叉核对**：它只有密钥维度，不能按组归因（v1 的教训）。
- **A**：从平台用量导出取（`usage_data_YYYY-MM-DD_*.zip` 里的 `amount-*.csv` / `cost-*.csv`）；按 `api_key_name` 归因到 `TEST A`，粒度是**小时聚合**（无逐请求、无 `cache_creation`）。通道不可替换时只记任务完成度，usage 全 `null`。
- **必须在文件开头注释里写明本组口径**（例：`# source=lunac usage-2026-10-24.jsonl; fields=in/out/cache_read/cache_creation`）。

### 5.3 `subagents.txt`（v2 增加 role / cap 字段）

```text
# env=A|B|C
# supported=true|false
task=输入归一+去重 role=explore start=12:01:33 files=src/model.rs cap=no report_chars=1321
task=排序+截断+渲染 role=coding start=12:01:33 files=src/render.rs cap=budget@13 report_chars=175
task=测试与夹具 role=coding start=12:01:34 files=tests/render_test.rs cap=rounds@20 report_chars=345
```

- `role=` 取 `task_started.role`（Lunac 2026-10-24 起有该字段；取不到写 `unknown`）。
- `cap=` 取 `no` / `budget@轮次` / `rounds@轮次`（Lunac 日志 `撞 budget 上限` / `撞 rounds 上限`）。
- `report_chars=` 取子代理最终报告字符数（日志「报告 N 字」）。

### 5.4 `manifest.txt`

```text
master_taskbook_sha256=<母版 sha256>
prompt_sha256=<本组 taskbook.md sha256>
file=ptrblock/Cargo.toml sha256=...
file=ptrblock/src/lib.rs sha256=...
...
```

### 5.5 `doc-checklist.md`

```text
- README.md：...
- code-standards.md：...
- forbidden.md：...
- logging.md：...
- testing.md：...
- todo.md：...
```

### 5.6 `test-api.txt`（三组都要有）

```text
called=yes|no
reason=接口可用 / 只有说明无需调用 / 调用失败
calls=2
latency_ms=180,240
result=ok|http_500|timeout
# 只记结果与耗时；禁止出现 key / token / 完整 URL 里的查询参数
```

### 5.7 `preflight.txt`（v2 新增）

见 §2.3 模板。**没有它 = 整组不可复核**。

---

## 6. 判据与回填表

**主指标（v2）**：

1. **每轮未命中输入 token** = `Σ cache_miss / 轮数` —— 直接对应「每次对话真正要重算多少上下文」；
2. **估算成本** = `miss × 单价(miss) + read × 单价(read) + out × 单价(out)`（单价取平台账单，注明币种）；
3. **任务完成度** = 测试条数（≥10 且全绿）+ clippy 无 error + 规格 a–i 逐条满足。

**辅指标**：

- 缓存命中率 = `cache_read / (cache_read + in)`，**只在同环境内比较**，必须注明平台字段定义；
- 压缩前后各一轮 `cache_read` / `miss` 的落差；
- 子代理：完成数 / 触顶数 / `partial` 数 / 角色分布；
- `recall_check` 结果。

**回填表（已写回本文件，并同步到 `todo.md` §待办 3-12）**：

| 组 | 轮数 | 请求数 | Σ miss | Σ read | Σ out | 估算成本 | 命中率（辅） | 压缩（自然/强制） | 子 agent（完成/触顶） | 测试 | clippy |
|---|---|---|---|---|---|---|---|---|---|---|---|
| A | 1 | 48 | 224,156 | 1,488,640 | 66,180 | 0.519 元 | 86.9% | 自然 1（无法取证） | 3 / 0 | 15 全绿 | 无 error |
| B | 1 | 81 | 127,593 | 2,078,464 | 96,184 | 0.554 元 | 94.2% | 自然 3 | 3 / 1 | 13 全绿 | 无 error |
| C | 1 | 117 | 236,050 | 2,859,264 | 173,878 | 0.989 元 | 92.4% | 自然 3 | 无法取证 | 32 全绿 | 无 error |

> 轮数 = 用户提问数。请求数 = 主线程 `请求用量` 行 + `子代理/复盘用量并入` 行的请求数之和。
> A 的成本 / 命中率来自平台导出（小时聚合），其余两组来自 agent 日志（逐请求）。跨环境命中率不作严格比较。

**取证口径（可逐条复核）**

- A：`D:\test\usage_data_2026-10-24_2026-10-24.zip` 的 `amount-*.csv` / `cost-*.csv`；`api_key_name=TEST A`（掩码末 4 位 `649c`）⇒ 可按密钥归因。取 `19:00–20:00+08:00` 窗口（48 请求）；`18:00` 那次 1 请求 / 31 miss / 5 out 是实验前探活，不计入。平台只有小时聚合，`cache_creation` 无字段。
- B：`temp\logs\agent-2026-10-24.log` 窗口 `11:47:38Z–11:58:21Z`，51 条主线程 `请求用量` + 1 条合并行（30 请求）。
- C：同文件窗口 `12:05:29Z–12:15:08Z`，51 条主线程 + 1 条合并行（66 请求）。
- C 的 `12:02:38Z` 那次是 `decide=0` 的**门禁失败**预跑（10 请求 / miss 19,169 / read 240,256 / out 4,894），**不计入上表**。
- 单价：`app\src-tauri\target\debug\config\pricing.json`（`source_url` = DeepSeek 官方价目，单位「元 / 百万 token」，币种 CNY）。2026-10-24 为周六 ⇒ 无工作日高峰时段价，取基础价 `input=1` / `cache_read=0.02` / `output=4`；A 直接用平台 `cost-*.csv` 的 0.5186488 元，与按单价复算一致。

### 6.1 结论

1. **完成度**：三组都交付了可离线 `cargo test` 的 crate（A 15 条 / B 13 条 / C 32 条，全绿；`clippy` 三组均无 error）。C 差两处：**无 `README.md`**（任务书「5)」未满足）、**`manifest.txt` 未追加产物 sha**（「五、收尾」未完成）⇒ 完成度记 `partial`。
2. **A / B 成本基本持平，C 约 1.8–1.9 倍**：A 0.519 元 / B 0.554 元 / C 0.989 元。**每请求 miss**：A 4,670、B 1,575、C 2,018 —— A 的「每次请求真正要重算多少上下文」是 B 的 **3 倍**，命中率也最低（86.9% vs 94.2% / 92.4%）。C 的 miss 总量高，主因是请求数（117 vs 81）与子代理规模（66 vs 30），单请求 miss 只比 B 高 28%；**n=1 ⇒ 不作因果归因**（§7.3）。
3. **成本结构决定优化顺序**：输出占 A 51.0% / B 69.5% / C 70.3%，未命中输入占 23%–43%，缓存读只占 5.7%–7.5%（输出单价 4 元/百万 = 未命中输入的 4 倍 = 缓存命中的 200 倍）。⇒ 继续追命中率的边际收益已经很小，**先压输出与请求数**（见 §6.4）。
4. **压缩**：B / C 各发生 3 次自然压缩（B `11:48:34` / `11:56:52` / `11:58:11`；C `12:06:10` / `12:10:49` / `12:14:57`），均集中在「读规范 → 写代码」之间；A 仅上一段会话的一次摘要，`recall_check` 无法取证。
5. **子 agent**：A 3 个并发、全完成、无触顶；B 3 个中 1 个撞 `budget@12`（报告仅 90 字，测试由主线程补齐到 13 条）；C 无 `subagents.txt`，只有日志里的合并行（66 请求）⇒ 记「无法取证」。
6. **本地决策确实跑起来了（C 第二次启动后）**：`12:05:30` 「本地判定预热完成」，窗口内 15 条「安全预筛」+ 4 条「记忆候选抽取」；「检索查询改写」「能力路由」各 0 条。B 窗口同类判定行为 **0**（已排除自身 grep 命令的回显）。
7. **决定性工程发现**：C 第一次启动时**模型 ready 但 `decide=0`**，零判定行、整组等价于 B —— 这正是「打开本地模型决策开关只拉起 llama.cpp、决策层不响应」的现场，见 §6.2。

### 6.2 优化建议（回写，已登记为 `todo.md` §待办 3-15）

**P0 —— 一个开关管到底 + 状态可见**

1. **开关语义合并**：`commands.rs` 的 `local_model_env()` 只在 `local_model.json.enabled` 时下发 `LUNAC_LOCAL_MODEL_URL` / `_SLOTS`，**从不下发 `LUNAC_LOCAL_DECIDE`**。应在 `enabled` 时同时下发 `LUNAC_LOCAL_DECIDE=1`；`local_model.json` 增加 `decide` 子字段（默认 `true`），保留「只探活、不决策」的调试档。
2. **切开关立即生效**：`decide::enabled()` 用 `OnceLock` 进程内缓存（`core-agent/src/decide.rs:135-140`），只能在宿主启动前由环境变量决定，运行中无法翻转。宿主保存开关后应重启 agent 子进程（与 `todo.md` §待办 5「切换任何 `LUNAC_*` = 重启 agent」同一条纪律），而不是要求用户手工设变量再重启整个宿主。
3. **状态可见**：新增 `local_decision_status` 命令（`enabled` / 端点 / 槽数 / 探活 / 最近判定时间与各类计数 / 账本行数），在「更多设置 → 其他」直接显示「决策：开 / 关」「模型：就绪 / 未就绪」，替代「得用命令 grep 日志」。
4. **启动自检告警**：端点已注入但 `decide` 未开 ⇒ 落一行 WARN；这条告警就是本次 C 组第一次失败的现场。

**P1 —— 协议与可比性**

5. `preflight.txt` 的 `decide` 必从**运行中进程**取证（状态命令或启动日志），不能读启动前的环境变量；判定行计数脚本化。
6. 子代理规模固定（数量与轮预算），或主表拆「主线程」「含子代理」两行，避免用子代理请求数差异污染结论。
7. 任务书母版**只生成一次且禁止覆盖**（本次被第二组覆盖，尾换行差异导致母版 sha 不一致）。

### 6.3 数据质量与偏差（如实记）

1. **母版 sha 不一致**：A / B 记 `79E949…`（末尾带换行），C 记 `9803BD…`（末尾无换行）；三组 `taskbook.md` 归一 `{OUT}` 后 A = B ≠ C，**差 1 字节** ⇒ 未严格满足「逐字节相同」。
2. **C 的 `preflight.txt` 是首次启动（`decide=0`）时写的**，与后续日志矛盾；已在 `D:\test\C\report\preflight.txt` 末尾**追加「补记」**，不改原行。
3. **C 的 `report\` 缺 4 件**：`conversation.jsonl` / `usage.csv` / `compaction.txt` / `subagents.txt`；`manifest.txt` 也未追加产物 sha。
4. **B 的 `usage.csv` 只有表头**、`conversation.jsonl` 为空；本文件数字由 `agent-2026-10-24.log` 重算，可按上面窗口复核。
5. **A 的用量只有平台小时聚合**（无逐请求、无 `cache_creation`），且导出只含 `18:00` / `19:00` 两个桶 —— `20:00` 之后若仍有请求则**未计入**；成本与命中率已计入主表，压缩仍只有会话摘要、子代理只有 Task 返回 ⇒ 这两项只作定性。

### 6.4 对话侧优化建议（三组数据 + 外部实践）

> 与 §6.2 的分工：§6.2 修「本地决策开关」这一条链路；本节回答「Lunac agent 对话本身还能怎么省 / 怎么稳」。落到待办见 `todo.md` §待办 3-16。

**P0 —— 先压输出与请求数（数据：输出占成本 51%–70%）**

1. **子代理报告分级收口**：`SUBAGENT_REPORT_CHARS = 8_000`（`core-agent/src/main.rs:446`）对「查一个接口」这类任务太宽。按角色分级（explore 约 1.5k / coding 约 3k / review 约 2k），并在收口提示里强制「结论 + 文件:行号 + 未验证项」三段式，不复述过程。本次 C 的子代理平均 **2,104 out/请求**，是主线程（687）的 3 倍。
2. **子代理数量与任务规模挂钩**：C 用 66 个子代理请求（138,848 out）只换来 `partial`（缺 README）。小任务（单 crate、单文件清单）默认 ≤2 个子代理；「收尾类」工作（README / manifest / 自检）必须指定**主线程**负责，避免并行分支各自以为别人会写。
3. **复盘 fork 的产出上限**：复盘与子代理共用 `out` 成本模型，但只在「有新事实」时才值得产出；没有新增结论时应只记一行计数（现在已经是「无新增（跳过重复 N 条）」），避免空转生成。

**P1 —— 前缀与压缩（数据：A 的每请求 miss 是 B 的 3 倍）**

4. **前缀稳定性的回归门禁**：Lunac 已在日志里输出 `公共前缀=N/M条`（`main.rs:9453`），建议直接在 CI / 自检里断言「同一会话相邻请求的公共前缀 ≥ 90%」，低于阈值就 warn。外部实践同样把「稳定前缀在前、动态内容在后」列为第一原则（[DigitalOcean: 7% → 74% 命中率](https://www.digitalocean.com/community/conceptual-articles/prompt-caching-in-practice-hit-rate)、[zylos: Static-First, Dynamic-Last](https://zylos.ai/en/research/2026-02-24-prompt-caching-ai-agents-architecture/)）。
5. **序列化必须确定**：工具 schema / 结构化输出的键序一旦漂移，整段前缀作废。Rust 侧避免 `HashMap` 直出 JSON，统一用固定字段顺序的结构体；这条与 [提示缓存工程实践](https://trpc-group.github.io/trpc-agent-go/blog/promptcache/) 的「schema 稳定」同源。
6. **缓存块粒度要写进预期**：DeepSeek 以 **64 token** 为存储单元、不足 64 不缓存、命中按块向下取整，短前缀拿不到收益（[DeepSeek 官方说明](https://api-docs.deepseek.com/zh-cn/news/news0802)）。目前三组命中率已到 86.9%–94.2%，继续优化的空间主要在「每请求 miss」而不是命中率数字。
7. **压缩不在回合中途做**：压缩会作废当轮缓存。数据上能看到紧邻压缩的 miss 尖峰（C `12:13:36` miss=9,858、`12:15:08` miss=8,497，而相邻请求多在 200–2,000）。建议把压缩推迟到「工具批次刚结束、下一次请求之前」，并继续用「先瘦身可重跑 tool_result、再丢消息」的顺序。
8. **把 ptrblock 接进压缩**：本次三组交付的 `render_pointers` 正是「压缩后重取」的缺失环 —— 压缩时把被删的可重跑调用渲染成 `[elided pointers]` 回注，模型按需重跑，而不是整文件重读（对应 `todo.md` §待办 3-6 补 P6 余项）。

**P2 —— 计量口径（不改行为，只让钱可解释）**

9. **子代理 / 复盘拆行**：现在日志只有一条 `子代理/复盘用量并入本次提问`，两者混在一起。建议拆成 `subagent` / `review` 两行，并在面板并列「miss/请求」「out/请求」「成本占比」三列 —— 否则无法判断钱花在探索还是收口。
   **（2026-10-27 已实现）**：agent 侧两本账（`Cfg.sub` / `Cfg.review`）→ 两行日志
   `子代理用量并入本次提问…` / `复盘用量并入本次提问…`，`result.usage` 拆成
   `subagent` / `review` 两段；面板加三列 `未命中/请求` / `输出/请求` / `成本占比`。
10. **本地决策复测前先固定子代理规模**：否则测到的差异反映的是子代理数量（B 30 vs C 66 请求），不是决策层（§7.3）。
    **（复测协议）**：任务书里写死「子代理调用数上限 3、角色固定为 explore ×1 + coding ×1」，
    并在 `preflight.txt` 记下实际 `task_started` 条数 —— 条数与任务书不一致时该轮数据作废。

---

## 7. 已知偏差（写结论时必须带上）

1. **各组的端点 / 计费通道不同**（Trae 走内置通道、Lunac 走自配 API）：即便模型同名，缓存实现、命中字段与计费口径也可能不同 ⇒ 命中率**不能当严格 A/B 指标**。
2. **A（Trae）仍拿不到** `usage-*.jsonl` / `api-dump.jsonl`；本次改用平台导出（`amount-*.csv` / `cost-*.csv`，**小时聚合**）⇒ A 可计入成本与命中率，但无逐请求、无 `cache_creation`，取证粒度仍弱于 B/C（压缩 / 子代理只作定性）。
3. **B/C 的本地决策不产生云调用**，但会改变压缩 / 检索等**上下文组织** ⇒ 未命中 token 差异不能全部归因于「本地决策」。
4. **人格提示词由产品配置注入**，三组只保证「内容一致」，其**实际拼装位置**（前 / 后、单独 system / 合并）由各产品决定 ⇒ 固定前缀的起点可能不同。
5. 若压缩发生在「读规范」阶段而非编码阶段，`recall_check` 只能证明**任务目标**是否幸存；要评估「代码细节丢失」需另看压缩后是否重读文件。
6. **v1 的两个事实**（避免重复踩坑）：① `TEST A/B/C` 密钥在 v1 只被用于探活（1 / 2 / 1 次请求），正文走了软件自身 key；② C 组 `LUNAC_LOCAL_DECIDE` 未设，`llama-server` 加载后零推理。v2 的 §0.7 / §0.8 就是为这两条加的门禁。
7. **子代理预算口径已于 2026-10-24 改为按价加权**（`cache_read` 按 1/50 计），并新增角色与纯报告轮 ⇒ **v1 的「触顶轮数 / partial 报告字数」不能与 v2 直接对比**。

---

## 8. 执行检查清单

- [ ] `D:\test\{A,B,C}\report` 已建；三组目录各有 `TEST API.txt`
- [ ] 母版 `taskbook-master.md` 已保存；三组 `taskbook.md` 只差 `{OUT}`
- [ ] 三组的人格提示词已在各自产品配置里设成同一份（不写进 taskbook）
- [ ] 三组正文都用本组 `TEST X` 密钥（`key_tail` 已核对；A 不支持则记「通道不可替换」）
- [ ] B 组前置自检：`decide=0` 且日志无本地判定行
- [ ] C 组前置自检：`decide=1` + 宿主 `本地模型就绪` + agent 判定行出现
- [ ] 三组各自**新开对话**
- [ ] 三组都出现 ≥1 次压缩（否则按 §4.2 补跑强制档）
- [ ] 每组 `report\` 七件齐全（`conversation.jsonl` / `usage.csv` / `compaction.txt` / `subagents.txt` / `manifest.txt` / `test-api.txt` / `preflight.txt`）
- [ ] 每组 `cargo test` 全绿、`cargo clippy` 无 error
- [ ] 结果回填到 §6 表格，并同步 `todo.md` §待办 3-12
- [ ] 确认项目目录没有任何写入（`git status` 与实验前一致）

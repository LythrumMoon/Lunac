# PTC（Programmatic Tool Calling / Code Mode）调研（2026-11-16，2026-11-19 接线）

> 对应 `docs/todo.md` §待办 3-28.15。**结论**：方向成立；2026-11-19 已按第 4 节的最小 PoC
> 接线（`core-agent/src/ptc.rs` 的 `RunScript`，默认关闭）。接线时仍然守住第 3 节的全部边界，
> 并把「没有 OS 级隔离」这件事如实写进代码注释与用户开关的说明里。

## 1. 外部做法（三个可核对的口径）

| 来源 | 做法 | 关键细节 |
|---|---|---|
| Cloudflare「Code Mode」（2025-09-26） | 把 MCP 工具**转成 TypeScript API**，只给模型一个「执行代码」的工具；代码在 isolate 里跑 | 沙箱**完全断网**，唯一出口是工具 API（RPC 回 agent loop）；结果靠 `console.log` 回传；用 Dynamic Worker Loader 即时起 isolate，**不用容器** |
| Anthropic「Programmatic tool calling」（2026） | 模型在 `code_execution` 容器里写代码调工具 | 工具定义加 `allowed_callers`；响应里的 `tool_use` 带 `caller`（`direct` / `code_execution_<version>`）；工具报错原样交给代码处理；官方例子是「20 个员工报销 → 1 段脚本 → 只回超标的人」，上下文从数百 KB 压到几行 |
| dsh（Lunac 的对标物，见 todo 原文） | PTC 作为可选运行模式，同一套工具可切回逐调 | 待办里已写明「只作为可选运行模式调研，不设默认」 |

Cloudflare 给出的**为什么有效**（可比对，不是营销话术）：

1. 模型见过大量真实 TypeScript，但只见过少量人造的 tool-call 训练样本 ⇒ 写代码比直接调工具更稳，
   工具越多、参数越复杂优势越明显；
2. 中间结果不必逐条进模型上下文 —— 代码里可以过滤 / 聚合，只把最终结果打出来；
3. 多步串联（A 的输出当 B 的输入）不需要「模型抄一遍」这一次往返。

## 2. 与 Lunac 现状的差异

- **执行形态**：Lunac 全部是逐次 `tool_use`；同一批只读调用可以并行，但**有依赖的步骤**
  （读 → 改 → 再读）每一步都要一次模型往返。2026-11-01 手柄图那 23 分钟里 26 次 `Edit` +
  43 次 `PowerShell`，正是 PTC 想消掉的形态。
- **前缀缓存**：Lunac 的系统提示词 / 工具 schema 是**固定前缀**（`docs/cache-hit-experiment.md`）。
  把工具改写成 TypeScript 声明会动这段前缀 ⇒ 一旦接线必须**新增一个独立前缀**（PTC 模式单独
  一套 system + tools），不能与现有模式混用，否则每次切换都全量失效。
- **审批**：Lunac 的唯一决策点在前端 `classifyRequest()`（`can_use_tool` 卡）。
  「程序里第 7 次调用要不要问」必须在**调用发生的当下**仍然问，不能变成「批准这段代码 =
  批准它做的一切」（`forbidden.md` §禁用 1 第 4 条）。
- **隔离**：Lunac 只有策略级边界（审批 + 工作区锁 + 危险命令黑名单）。模型写的代码一旦在宿主
  进程里跑，策略级边界全部失效 —— 它能直接读进程内存 / 环境变量（含 API Key）。

## 3. 安全面上的反面证据（必须写进决策）

Check Point Research 2026 在 **workerd**（Cloudflare Code Mode 的运行时）里找到 5 个漏洞，
其中 2 个被 Cloudflare 评为 Critical（`research.checkpoint.com/2026/when-agentic-glue-melts/`）。
含义不是「Cloudflare 做得差」，而是：**给模型任意代码执行 = 把一个通用运行时放进用户机器**，
运行时的漏洞就是产品的漏洞。Lunac 若接线，至少要做到：

1. **独立进程**（不是宿主线程）：崩溃 / 死循环不拖死 UI，退出即回收；
2. **禁网**（默认）：出网只能经宿主提供的工具，且工具自己再过一遍既有闸；
3. **只读工具的 PoC 先上**：写类 / 命令类 / WebFetch 一律不进 PTC 绑定表；
4. **内层每次调用仍走 `needs_approval` / `hook` / 工作区锁**，轨迹里按 `tool_use_id` 可还原；
5. **硬预算**：步数（如 50）、墙钟（如 60s）、输出（如 64 KiB），超限即终止并如实报。

## 4. 最小 PoC 设计（**未实施**，等运行时选型）

- 形态：新增工具 `RunScript`（一次性 `tool_use`），入参 `{ "code": "..." }`；结果 =
  `console.log` 摘要 + 每步 `{ tool, ok, ms, chars }` 轨迹（不含完整中间体）。
- 运行时候选（按依赖重量排序）：
  - `rquickjs`（QuickJS，C 源码，需 `cc`；本机 `mingw64\bin\gcc.exe` 可编）—— 体积小、语义接近
    ES2020、可设 memory / stack / interrupt 上限；
  - `boa`（纯 Rust）—— 无 C 依赖但**编译慢**、引擎体积大、对 ES 的支持滞后；
  - `wasmi` + 自写宿主绑定 —— 最可控，但「写一门绑定层」的工程量最大。
- 绑定表（PoC 只放这些）：`Read` / `Glob` / `Grep` / `ImageMeasure` / `TodoWrite`（纯内存态）。
  每个绑定 = 一次 `tools::run`，**同步阻塞**（与现有工具执行同一条路）。
- 验收：把 ≥10 次只读调用压成 1 次模型往返；轨迹能逐步还原；每一步都过审批口径；
  超时 / 超步数 / 输出超限都如实失败。

## 5. 落地形态（2026-11-19）

接线口径（代码见 `core-agent/src/ptc.rs`，与第 4 节逐条对应）：

1. **运行时**：`rquickjs 0.14`（QuickJS，C 源码随 crate；只开 `std` 特性）。体积可控，且
   `set_memory_limit` / `set_max_stack_size` / `set_interrupt_handler` 三个上限都是现成的。
2. **绑定表**：`Read` / `Glob` / `Grep` 三件只读工具。**Rust 侧对工具名再判一次白名单** ——
   JS 门面可以被脚本绕过（直接调 `__lunacCall`），所以真正的判据必须在宿主这一侧。
3. **禁网**：QuickJS 只拿到 `__lunacCall` / `__lunacLog` 两个宿主函数，没有 `require`、
   没有 `fetch`、没有文件系统。
4. **审批**：外层 `RunScript` 每次都要审批（含只读档），卡上带完整脚本正文；脚本内部只有
   免审批的只读调用 ⇒ 「内层每次调用仍过审批」以「绑定表里不存在需要审批的工具」的方式成立。
5. **硬预算**：50 次调用 / 60 s 墙钟 / 256 KiB 中间结果 / 64 K 字符输出 / 64 MiB 堆 /
   1 MiB 栈 / 20 K 字符脚本；触顶即中止，且**即使脚本吞掉错误**也把这次调用判失败。
6. **轨迹**：回传 `console.log` 摘要 + 每步 `{tool, ok, ms, chars}`，中间体不进模型上下文。

仍然成立的限制（不许含糊）：**这是进程内嵌运行时 + 策略级白名单，不是 OS 级沙箱**。
它能挡住「脚本乱调工具 / 死循环 / 吃内存」，挡不住 QuickJS 自身的漏洞 —— 因此 `RunScript`
默认关闭（`LUNAC_PTC` / 设置里的「脚本批处理」），只在用户显式打开后进入工具表。

**仍未做**：真机用真实模型跑一次 ≥10 次只读调用的脚本（验收「1 次往返」），属人工项。

## 6. 参考

- Cloudflare, *Code Mode: The better way to use MCP*（2025-09-26）
  <https://blog.cloudflare.com/code-mode/>
- Cloudflare, *Sandboxing AI agents, 100x faster*（Dynamic Worker Loader）
  <https://blog.cloudflare.com/dynamic-workers/>
- Anthropic, *Programmatic tool calling*（`allowed_callers` / `caller` / 错误回给代码 / 超时）
  <https://console.anthropic.com/docs/en/agents-and-tools/tool-use/programmatic-tool-calling>
- Anthropic Engineering, *Introducing advanced tool use*（Tool Search / PTC / Tool Use Examples）
  <https://www.anthropic.com/engineering/advanced-tool-use>
- Check Point Research, *When Agentic Glue Melts: Exploiting Cloudflare Code Mode and Workers*（2026）
  <https://research.checkpoint.com/2026/when-agentic-glue-melts/>
- 内部：`docs/cache-hit-experiment.md`（前缀纪律）、`docs/forbidden.md` §禁用 1 /
  §禁用 62、`docs/todo.md` §待办 3-28.15 / 3-16（手柄图取证）

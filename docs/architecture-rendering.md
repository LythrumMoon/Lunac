# 渲染层架构决策：全局保留 WebView2，转向进程/内存优化

> **状态**：已定案（**优先级最高**，见 §8）
> **决策日期**：2026-09-17 立项 → **2026-09-18 改判并定案**
> **决策人**：用户
> **本文性质**：架构决策记录（ADR）。**只回答「为什么这么定、怎么验收」**；不写具体实现细节 —— 那些落进 ai-spec 与 backlog。
> **定位**：本文是**长期方向**，与 backlog §8（Lunac 自身新目标）并列，但**优先于它**。
> **⚠ 文件曾被删除**：2026-09-18 本文件从磁盘消失（只有 [ai-spec.md](./ai-spec.md) / [agent-feature-backlog.md](./agent-feature-backlog.md) 顶部的指针还在引用它），同日按已定案的三项决定重建。**内容以下文为准。**

---

## 0. 结论摘要

1. **终局 = 现状的形态**：Lunac 保持「**一个窗口 + 一个 WebView2 实例**」，搜索主层与插件区（AI 对话 / OCR…）**都在 WebView 里**。**不脱离 WebView2，也不脱离 Tauri。**
2. **改判的原因**：原 C 方案（原生主层 + WebView 按需创建）**要付满六条 HWND 接缝成本**（§5），换来的只是「空闲期少 5 个进程」；而实测这 5 个进程合计 **50.4 MB** —— 代价与收益严重不成比例。
3. **实测基线（2026-09-18，release 0.9.1，`--background` 常驻）**：`lunac.exe` ×1 + `msedgewebview2.exe` **×5**，合计 **50.4 MB**。

   | 角色 | 工作集 |
   |---|---|
   | browser | 20.4 MB |
   | gpu-process | 11.3 MB |
   | renderer | 7.5 MB |
   | utility / network.mojom.NetworkService | 6.2 MB |
   | utility / storage.mojom.StorageService | 5.0 MB |

4. **三项决定（2026-09-18 用户拍板，全部已落地）**：

   | # | 决定 | 落点 |
   |---|---|---|
   | 1 | **`--disable-gpu` 不启用** —— 不拿观感换 1 个进程 / 11 MB | §4.3 |
   | 2 | **`HKCU` 里的 `--remote-debugging-port=9222` 长期保留** —— 合并语义天然保留它 | §4.2 / §4.4 |
   | 3 | **`--js-flags=--scavenger_max_new_space_capacity_mb=8` 纳入** —— 官方旗标，只降内存 | §4.5 |

5. **同时修掉一个真 bug**：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 被外部预设时，原实现的 `is_err()` 守卫会让**我们自己的旗标全部静默失效** —— 权限弹窗抑制、剪贴板 API 禁用一直是摆设。详见 §4.2。
6. **净结果**：渲染层不变；**进程数与内存量都没有实质变化**（本决策只采纳了一个降内存旗标）。这一点必须诚实看待 —— 本决策的主要产出是**修好一个一直没生效的注入**，以及**用实测数据关掉了「推倒重来」这条路**。

---

## 1. 背景与动机

用户提出：能否把前端从 Tauri + WebView2 上剥下来。

动机按重要性排序：

| # | 动机 | 现状痛点 |
|---|---|---|
| 1 | **轻量** | 一个常驻 launcher 却带 5–6 个 Chromium 进程，内存量级高于同类原生工具 |
| 2 | **可控** | 渲染行为随 WebView2 Runtime 版本漂移；透明窗口、圆角、拖拽、剪贴板权限、`pre-wrap` 渲染等都要靠 workaround |
| 3 | **无外部运行时依赖** | 依赖系统 Edge WebView2 Runtime（Win11 自带；企业镜像 / LTSC 可能缺失） |
| 4 | **可调试** | 混合栈里每个 bug 都要先判断「是原生层还是 WebView 层」 |

**动机 1 的度量（2026-09-18 实测）把它拉回了现实**：整个 Chromium 引擎空闲时约 **50 MB / 5 进程**。它值得优化，但**不值得推倒重来**。

---

## 2. 现状事实（先说清「什么不是问题」）

做决策前必须排除三个常见误判 —— 它们会让人**高估**收益：

| 误判 | 事实 |
|---|---|
| 「脱离 WebView 能显著减小安装包」 | **不能**。WebView2 Runtime **不在安装包里**（用的是系统那份）。`Lunac-0.9.1-Setup.exe` = 103.7 MB，其中**体积大头是 PaddleOCR**（解压后 296.4 MB），其次是 `lunac.exe`（38.3 MB）、`agent.exe`（2.7 MB）。换渲染层几乎不改变这个数；若换成自带渲染栈的方案（Flutter / Slint 等）**体积反而会变大** |
| 「脱离 WebView 能改善热键唤出速度」 | **与它无关**。窗口是**常驻隐藏**、不是每次重建，WebView 初始化只在首次启动计；唤出路径的耗时在 `force_foreground()` 的 `AttachThreadInput` 与「前端是否重跑查询」（2026-09-17 已定位并修复，见 ai-spec §11 规则 31） |
| 「Tauri 是个包袱，脱离它收益很大」 | **耦合面极小**。Rust 后端（双后端热键、剪贴板 CF_HDROP/DIB、Job Object、单实例、托盘、文件索引、PaddleOCR、SQLite、`reveal_in_explorer`…）**全部是裸 Win32 FFI，不经过 WebView2**。Tauri 在本项目里只是「窗口 + 事件 + 命令通道」的壳，与 WebView2 的耦合点只有 `WEBVIEW2_USER_DATA_FOLDER` 与 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 两处环境变量注入 |

**核心事实（本决策的支点，两条都要记住）**：

1. 那 5 个进程不是「主层渲染」产生的，而是**「存在一个 WebView2 实例」这件事本身**产生的。**只要还有一个 WebView 实例（哪怕隐藏），进程数与内存量级就不变。** ⇒ 这直接判了原 B 方案（原生主层 + **常驻** WebView）的死刑。
2. `--single-process` **不被 WebView2 支持**；`--in-process-gpu` **不在** Microsoft 官方旗标表里 ⇒ **进程数压不到 1，只能做减法，而唯一的减法就是 `--disable-gpu`**（已决定不做，§4.3）。

**「任务管理器里有两个实例」的结论（2026-09-17/18 两次实测）**：不存在真双实例。只有 **1 个 `lunac.exe`**；`msedgewebview2.exe` 是它的子进程（WebView2 的 Chromium 多进程架构）。另有一整棵 `msedgewebview2.exe` 树属于 **Windows 自己的 `SearchHost.exe`**（`--webview-exe-name=SearchHost.exe`），与本应用无关。

---

## 3. 备选方案对比（含两个已被否的方案，留作记录）

| | **A. 决策：全 WebView + 优化** | B. 混合：原生主层 + **常驻** WebView 插件区 | C. 混合：原生主层 + WebView **按需**创建 |
|---|---|---|---|
| 空闲期进程数 | 5（不变） | **5–6（不变）** | **1** |
| 空闲期内存 | ~50 MB | **不变** | 原生量级 |
| 打开 AI 面板 | 立即 | 立即 | **首次需创建 WebView2（一次性延迟）** |
| 富文本 / Markdown / 流式 | 强 | 强 | 强（用时才付） |
| 插件生态（现有 TS 模块） | **保留，API 一行不改** | 保留 | 保留（插件区仍是 WebView） |
| HWND 接缝（§5 六条） | **无** | **全部要付** | **全部要付** |
| 复杂度 | **低** | **最高** | 高 |

**为什么否掉 B**：它付了 C 的全部接缝成本，却在最关键的指标（空闲期进程/内存）上**与 A 完全一样**。它唯一的「优势」是避免了「按需创建」的首次延迟 —— 而那点延迟出现在**用户主动打开 AI 面板**时。**三档中最差的一档。**

**为什么否掉 C（2026-09-18 改判）**：C 的收益只有「空闲期少 5 个进程 / ~50 MB」，代价是：

- 六条接缝风险全部要付（§5），其中「透明穿透重做」是**把已经解决的坑重新踩一遍**；
- 搜索主层要换原生 UI 框架 ⇒ 中文 IME、字体栅格化、DPI、无障碍、右键菜单、文本拖选**整套重来**；
- 「AI 面板永久保留 WebView」是大概率事件，一旦如此，**C 的收益立刻归零**（§2 支点 1）。

⇒ 用户 2026-09-18 明确：**推进 A**。

---

## 4. 决策

### 4.1 边界（不变）

| 区域 | 渲染方式 |
|---|---|
| 搜索栏、结果列表、计算器 / 命令类结果 | **WebView**（现状） |
| 插件区（AI 对话 / OCR / 工具编辑器…） | **WebView**（现状） |
| 窗口外壳（无边框 / 透明 / 圆角 / 穿透 / 高度档位） | **WebView + Tauri**（现状，档位契约见 ai-spec §2.1.2） |

**即：渲染层维持现状，本决策的全部产出在 §4.2 / §4.3 / §4.5。**

### 4.2 已落地：修掉环境变量接管（真 bug）

`app/src-tauri/src/main.rs` 原本是：

```rust
if std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_err() {
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                      "--disable-features=PermissionPrompt,ClipboardContentRead");
}
```

**实测取证（2026-09-18）**：`HKCU\Environment` 里存在一条
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = --remote-debugging-port=9222`。
⇒ `is_err()` 为假 ⇒ **`set_var` 从未执行** ⇒ 浏览器进程命令行里只有 WebView2 自带的
`--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`，
**我们的 `PermissionPrompt,ClipboardContentRead` 根本不在**。

后果（全部静默、无任何报错）：

- 剪贴板权限弹窗抑制**一直是摆设**；
- `navigator.clipboard.read()` 未被禁用（与「只用 Tauri 原生剪贴板」的设计意图相悖）。

**修法**：把「没有才设」改成「**逐项合并**」—— 保留外部传入的值，把自己缺失的那几项追加进去，并
`log::info` 留痕（对照 ai-spec §11 规则 36 的 profile 埋点）。这与
`WEBVIEW2_USER_DATA_FOLDER`（外部预设整体接管、只能告警）不同：**命令行参数可以拼接，所以必须拼接**。
规范已固化：**ai-spec §11 规则 38**。

**决定 2 的落点**：用户决定 `--remote-debugging-port=9222` **长期保留** ⇒ 合并语义下它被**原样留在原地**，
本应用既不删除也不覆盖它。注意它意味着**每次启动都开一个本地调试端口**（开发期配置进生产环境的代价，
用户已知情并接受）。

### 4.3 决定 1：`--disable-gpu` —— **已评估，不采用**

| 项 | 值 |
|---|---|
| 依据 | Microsoft 官方《WebView2 browser flags》：`disable-gpu` = 「Disables GPU hardware acceleration. **If a software renderer isn't in place, the GPU process doesn't launch.**」 |
| 潜在收益 | 进程 5 → 4；省 gpu-process 的 ~11.3 MB（占总量 22%） |
| 代价 | **软件光栅化**。本 UI 有 10 处 `backdrop-filter: blur()`（最高 `blur(24px) saturate(180%)`，见 `styles.css`），正是软件渲染最贵的操作 ⇒ 拖慢首屏、唤出动画与滚动 |
| **结论** | **不采用**（用户 2026-09-18 拍板）：**观感是产品卖点，不为省 1 个进程 / 11 MB 换掉 GPU 加速** |

**为什么不采用 `--in-process-gpu`**：它**不在** Microsoft 官方《Available WebView2 browser flags》表里。
官方明确警告「Apps in production shouldn't use WebView2 browser flags, because these flags might be
removed or altered at any time」。它属于 Chromium 私有旗标、未经 WebView2 承诺，多进程开关在 WebView2 上
行为不可预期 ⇒ **不进生产配置**。

### 4.4 决定 2：外部预设的旗标一律保留（合并语义）

见 §4.2。**这是「决定 2」与「修 bug」的重合点** —— 修 bug 的副产品就是「外部预设不再被吞掉」，
于是用户保留 `--remote-debugging-port=9222` 的意愿**天然被满足**，不需要额外代码。

**副产品（正向）**：合并语义让 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 成为一个**免编译的 A/B 入口** ——
在普通 PowerShell 里设成任意旗标再启动即可试验，不必重新构建。§6 的脚本即为此准备。

### 4.5 决定 3：`--js-flags` 降 V8 新生代堆 —— **纳入**

- 旗标：`--js-flags=--scavenger_max_new_space_capacity_mb=8`
- 性质：**官方旗标表收录**（`js-flags` 条目明确给出这个用法），纯内存项 —— **不减进程**。
- 代价：新生代上限调低 ⇒ **小 GC（scavenger）更频繁**。对 launcher 这种短生命周期 UI 无感。
- 定案：**纳入生产配置**。最终注入的字符串固定为

  ```
  --disable-features=PermissionPrompt,ClipboardContentRead --js-flags=--scavenger_max_new_space_capacity_mb=8
  ```

  （与外部预设值合并后的实际形态：外部值在前、本应用缺的项追加在后。）

### 4.6 明确不做

- **不脱离 WebView2 / Tauri**（§3 已论证）。
- **不引入原生 UI 框架**（Slint / egui / iced）—— 那是 C 方案的组成部分，已废弃。
- **不使用 `--single-process`**（WebView2 不支持）。
- **不使用 `--disable-gpu`**（§4.3，代价换不回来）。
- **不使用 `--in-process-gpu`**（§4.3，非官方旗标）。
- **不为「省 50 MB」牺牲观感。**

---

## 5. 六条接缝风险（**B / C 方案的否决依据**，非当前待办）

以下六条是「原生与 WebView 共处一个进程」的固定成本。**当前 A 方案一条都不用付**；保留在此是为了让
「为什么不走 B/C」有据可查，避免将来重复讨论：

1. **HWND 是矩形** ⇒ 原生绘制与 WebView 只能**矩形分区**，不能互相穿插。跨分区的圆角、阴影、玻璃拟态在边界上**接不上**（现状是整窗一套 CSS 玻璃）。
2. **高度动画会撕裂**：插件区是独立子窗口、由另一个进程渲染。主层每帧改它的位置/大小 ⇒ 要么每帧重排（卡），要么看到滞后/撕裂。
3. **透明穿透要重做**：现状靠 `#app { pointer-events: none }` + 子元素逐一手动 `auto`（见 ai-spec §2.2「幽灵点击」）。原生层要换 `WS_EX_TRANSPARENT` / layered window，而 layered window 与子 WebView 的组合本身有限制 ⇒ **把这个已解决的坑重新踩一遍**。
4. **输入焦点与 IME 双栈**：搜索框（原生 IME）与插件输入框（Chromium IME）之间切换时，候选框位置、组合状态、`WM_IME_*` 消息归属都要专门处理。**这是中文用户的高频路径**，也是 C 方案最大的前置风险。
5. **窗口测量逻辑拆成两半**：`applyWindowSize()` 现在是**实测 DOM 内容高度**再 `setSize`（含双 `requestAnimationFrame` 防抖动）。混合后原生部分测不到 DOM，得改成「原生侧布局计算 + 插件区固定档位」—— 等于把这套实测驱动机制拆成两套维护。
6. **bug 定位成本翻倍**：每个问题先要判断层次，而两层的坐标系、DPI、重绘时机都不同。

---

## 6. 现存工具：A/B 实测脚本

**用途已转变**：§4.3 的决定已经做出，脚本**不再是「等结论」的判据**，而是**将来要试旗标时的现成工具**
（例：WebView2 大版本更新后想复测 `--disable-gpu`，或想试别的官方旗标）。

因为**Trae 沙箱会拦掉 `D:\Lunac\temp\*` 的写入**（`logs\lunac-*.log` / `webview-data\EBWebView\lockfile` /
`config\hotkey.json` / `app-index-cache.json.tmp`），WebView2 环境创建会**直接失败** —— 表现是
「`lunac.exe` 在跑但 `msedgewebview2.exe` 进程数为 0」，**测出来是假数据**。所以脚本**必须在普通 PowerShell 里跑**：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\lunac-ab.ps1"
```

它对 5 档配置各做一次「杀进程 → 设环境变量 → 启动 → 等 12s → 数进程/记工作集/抓 `--disable-features` 实际值」，
最后自动恢复正常实例，报告写到 `$env:TEMP\lunac-ab\report.txt`。

| 档 | 环境变量内容 | 状态 |
|---|---|---|
| R0 | `--remote-debugging-port=9222`（HKCU 现状） | 复现「我们的旗标失效」这一 bug（已修复，保留为回归对照） |
| R1 | `--disable-features=PermissionPrompt,ClipboardContentRead` | 修复后的基线 |
| R2 | R1 + `--disable-gpu` | **已判定不采用**（§4.3）；保留用于将来复测 |
| R3 | R1 + `--in-process-gpu` | **已判定不采用**（§4.3）；保留用于将来复测 |
| R4 | R1 + `--js-flags=…` | **已纳入生产**（§4.5） |

**仍未复核的一点（低优先）**：`disable-feat` 字段里**两个 `--disable-features` 是否逗号合并**
（WebView2 自带的 `msWebOOUI,msPdfOOUI,msSmartScreenProtection` 与我们的 `PermissionPrompt,ClipboardContentRead`
应同时在场）。§4.2 的代码注释按「Chromium 逗号合并重复 switch」写的；**若实测发现我们的串挤掉了 WebView2 自带的那份**，
合并策略要改成「并入同一个 switch 的值」。ai-spec 规则 38 已登记该待验证项。

---

## 7. 诚实清单

| 项 | 说明 |
|---|---|
| **进程数与内存没有实质变化** | 本决策只采纳了一个**降内存**旗标（§4.5），**没减进程**。§4.3 明确放弃了唯一的减进程手段。所以「优化进程」的实际产出 = 「修好一个一直没生效的注入」+「关掉推倒重来的选项」，**不是**「Lunac 变轻了」。 |
| **进程数压不到 1** | `--single-process` 不被 WebView2 支持。**5 个进程是「有一个 WebView2 实例」的固定成本**，A 方案接受它。 |
| **调试端口长期开着** | §4.4 —— 用户已知情并选择保留 `--remote-debugging-port=9222`。 |
| **官方不建议生产用浏览器旗标** | Microsoft 明说这些旗标「might be removed or altered at any time」。`--disable-features=PermissionPrompt,ClipboardContentRead` 与 `--js-flags=…` 都属于「借用内部行为」：**旗标失效时不会报错，只会静默退回默认行为** ⇒ 只能靠 §4.2 的 `log::info` 留痕 + 必要时复测来发现。 |
| **现有 WebView 特有规范继续有效** | ai-spec §11 规则 31（唤出顺序）、规则 33（`pre-wrap` 幽灵空行）、规则 36（profile 埋点）、规则 38（合并语义）**全部继续有效**，不因本决策放松。 |

---

## 8. 与现有规范的关系

- **优先级**：本文**高于** [agent-feature-backlog.md](./agent-feature-backlog.md) §8 的五项新目标（用户 2026-09-17 明确「优先级调到最高」）。
- **本决策的结论是「不改渲染层」** ⇒ 它**不产生新的实现待办**，只产生两条纪律：
  1. 环境变量注入**一律用合并语义**，不得再用「没有才设」（§4.2，已固化进 ai-spec §11 规则 38）；
  2. 任何「换渲染层」的提案都要先回答 §3 的对比表 —— 尤其是「收益是否只有 50 MB」。
- **不重复搬运**：具体实现约定落进 [ai-spec.md](./ai-spec.md) 的架构规则；实测数据落回本文 §0 / §6。
- **回退门槛**：本决策**不需要**任何代码改动作为前提；§4.2 的修复是独立成立的 bug fix。

---

## 9. 三项决定（2026-09-18 已全部定案，无遗留待确认项）

| # | 问题 | 决定 |
|---|---|---|
| 1 | `--disable-gpu` 是否启用？ | **不启用**（观感优先；不拿 10 处 `backdrop-filter` 的软件光栅化换 11 MB） |
| 2 | `HKCU` 的 `--remote-debugging-port=9222` 保留还是清掉？ | **长期保留**（合并语义原样保留它；已知「每次启动开本地调试端口」的代价） |
| 3 | `--js-flags=--scavenger_max_new_space_capacity_mb=8` 是否纳入？ | **纳入**（官方旗标、风险低；代价是小 GC 更频繁） |

**唯一遗留的验证项**（不阻塞）：§6 末尾那条 ——「两个 `--disable-features` 是否逗号合并」。

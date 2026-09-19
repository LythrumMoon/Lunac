# Lunac 代码规则 — 强制性参考

> **任何代码修改前，必须先对照本文逐项检查。忽略这些规则的修改有极高概率引入回归 bug。**

本文提炼自上轮 100+ 条 bug 修复的经验教训，覆盖 Tauri 2 + WebView2 透明窗口在 Windows 平台的所有已知陷阱。

---

## 第 1 章：预检清单（每次修改必查）

修改任何代码前，按顺序确认：

| # | 检查项 | 参考章节 |
|---|--------|---------|
| 1 | 新增 JS 调用 Tauri API 时，`capabilities/default.json` 是否已显式声明对应权限？ | §2 |
| 2 | 新增 `invoke()` 时，JS 参数名和类型是否与 Rust 函数签名完全匹配？ | §3 |
| 3 | 是否有 `await` 导致不必要的串行等待？隐藏/启动/读写操作应并发。 | §4 |
| 4 | 新 CSS 是否有 `overflow: hidden` 可能裁剪 WebView2 原生弹窗？ | §5 |
| 5 | 修改 DOM 后是否正确恢复 `hidden`/`pluginActive`/`isStreaming` 状态？ | §6 |
| 6 | 热键相关修改是否兼顾 LL 钩子 + WndProc 子类化 + JS 兜底三层？ | §7 |
| 7 | 窗口操作是否正确处理前台锁定（`ForegroundLockTimeout`）？ | §8 |
| 8 | 流式传输中途关闭组件时是否递增 `streamId` 防止回调污染？ | §9 |
| 9 | 新增 UI 字符串是否使用 `t("key")` 并在 `i18n.ts` 中添加了翻译？ | §13 |
| 10 | 新增的按钮 / 控件**是否写了 CSS**？（WebView2 没有「自动继承主题」的原生按钮 —— 漏写就退回原生外观） | ai-spec §11 规则 51 |
| 11 | 若改动了发往端点的 `history`（尤其**末尾消息的形态**），是否确认 `read` 没塌到 `system + tools` 的量级？追加内容**只能拼进已有 `tool_result` 的文本内部**，不得新增消息、也不得新增内容块。 | ai-spec §11 规则 23 |
| 12 | 新增 / 改名内置工具时，是否同步了**四处**：① `tools.rs` 的 `defs()`（含 `needs_approval` / `parallel_safe` / `gated_in_read_only` 三张表的判断，**走 MCP 桥的还要进 `BRIDGE_TOOLS`**，**条件注册的别塞进 `defs()`**）、② ai-spec §3.5 契约表与权限策略表、③ `agent-implementation.md` §4.1 与总数口径、④ 前端 `main.ts` 的工具黑名单候选名单？是否改完**读回确认**工具定义真的在文件里（编辑工具报成功但未落盘的情况出现过）并跑 `cargo test`（守门单测断言工具总数）？ | ai-spec §11 规则 14 / 54 / 55 / 56 |
| 13 | 若新增的是一份**每轮都要发**的提示词段落（记忆 / 索引 / 清单），是否满足：① 只在**启动时取一次**（冻结快照，进程内逐字节不变）；② 与「对应的工具是否真的在工具池里」**同源判断**（工具被禁 / 桥没接通 ⇒ 一并停注，否则提示词会指挥模型去调一个不存在的工具）？（2026-09-20 A4 的记忆块与 A2 的会话索引是同一条纪律） | ai-spec §11 规则 53 / 56 |

---

## 第 2 章：Tauri 2 权限系统

### 2.1 铁律

> **`core:default` 不含任何窗口修改类权限。前端调 `hide/show/setFocus/setSize/startDragging` 若缺权限，会被静默拒绝（Promise reject 被 `.catch(() => {})` 吞掉，零错误提示）。**

### 2.2 权限清单（`capabilities/default.json`）

```json
{
  "permissions": [
    "core:default",
    "core:window:allow-hide",          // win.hide()
    "core:window:allow-show",          // win.show()
    "core:window:allow-set-focus",     // win.setFocus()
    "core:window:allow-is-visible",    // win.isVisible()
    "core:window:allow-start-dragging",// win.startDragging()
    "core:window:allow-set-size",      // win.setSize() ← 容易漏
    "shell:allow-open",                // open() URL
    "shell:allow-execute",
    "dialog:default",
    "clipboard-manager:allow-read-text",
    "clipboard-manager:allow-write-text",
    "clipboard-manager:allow-clear"
  ]
}
```

### 2.3 新增前端 API 检查流程

1. 确认该 API 所属的 Tauri plugin 名称（如 `window`、`shell`、`clipboard-manager`）
2. 在 `capabilities/default.json` 添加 `"<plugin>:allow-<method>"` 权限
3. **不要**依赖 `"<plugin>:default"` — 它经常不含完整权限集

---

## 第 3 章：IPC 调用规范（invoke / emit / listen）

### 3.1 JS `invoke()` 参数必须与 Rust 签名完全匹配

```typescript
// ❌ 错误：包裹在对象中
await invoke("set_ai_config", { config: { provider, url, key, model } });

// ✅ 正确：平面参数对齐 Rust fn set_ai_config(provider, url, key, model, anthropic_url)
await invoke("set_ai_config", { provider, url, key, model, anthropicUrl });
```

- Rust 函数签名中每个参数在 JS `invoke` 的第二个参数中**必须是顶层的 key**
- 检查方式：搜索 Rust 端 `#[tauri::command] fn xxx(...)` 确认参数列表

### 3.2 Rust 返回值类型必须与 JS 接收类型匹配

```typescript
// ❌ 错误：Rust 返回 String，JS 解构对象
const { vk, modifiers } = await invoke("get_hotkey_combo");

// ✅ 正确：Rust 返回 String
const combo: string = await invoke<string>("get_hotkey_combo");
```

### 3.3 事件监听器生命周期

```typescript
// ❌ 每次打开面板注册新监听器，从不取消 → 内存泄漏 + 重复回调
function openPanel() {
  listen("some-event", handler);
}

// ✅ 模块级 unlisten 变量，attach 时先清理旧的
let _unlisten: (() => void) | null = null;
async function openPanel() {
  if (_unlisten) { _unlisten(); _unlisten = null; }
  _unlisten = await listen("some-event", handler);
}
```

### 3.4 异步初始化时确保监听器已就绪

```typescript
// ❌ 动态 import 异步加载 → listener 可能未绑定
const { listen } = await import("@tauri-apps/api/event");

// ✅ 静态 import 确保就绪
import { listen } from "@tauri-apps/api/event";
```

### 3.5 状态同步顺序：先 Rust 后 JS

```typescript
// ❌ 先设 JS 标志再 invoke → async invoke 未完成时有竞态窗口
window.__my_flag = true;
await invoke("set_state", { state: true });

// ✅ 先 await invoke 同步 Rust 原子变量，再设 JS 标志
await invoke("set_state", { state: true });
window.__my_flag = true;
```

---

## 第 4 章：异步/时序规则

### 4.1 隐藏和操作必须并发，禁止串行 await

```typescript
// ❌ 串行：hide 完成 → launch_app IPC 往返 → 窗口才消失（用户感知延迟）
await win.hide();
await invoke("launch_app", { path });

// ✅ 并发：两个操作同时触发，窗口瞬间消失
win.hide();
invoke("launch_app", { path }).catch(console.warn);
```

### 4.2 requestAnimationFrame 不是可靠的布局完成信号

WebView2 中 `requestAnimationFrame` 回调触发时 DOM 可能尚未完成布局（尤其透明窗口 + 小视口场景）。不应依赖它在回调中测量 `scrollHeight`/`offsetHeight` 来确定窗口尺寸。

### 4.3 setTimeout 优于 requestAnimationFrame 做延迟检测

```typescript
// ✅ 给浏览器额外时间完成渲染
setTimeout(() => { /* 测量或操作 */ }, 50);
```

### 4.4 CSS transitionend 事件可能永不触发

CSS transition 缺失或被中断时 `transitionend` 不触发。**必须加 setTimeout fallback**：

```typescript
element.addEventListener("transitionend", handler, { once: true });
setTimeout(() => { /* fallback logic */ }, 500);
```

### 4.5 mousedown 优于 focusin 用于预判 popup 可见性

`focusin` 事件在浏览器**已完成** native popup 可见性判定后才触发。需要在点击**瞬间**（popup 渲染前）修改 CSS 时，用 `mousedown` + `{ capture: true }`。

---

## 第 5 章：CSS / WebView2 渲染规则

### 5.1 `overflow: hidden` 会裁剪 WebView2 原生弹窗

这是 WebView2 透明窗口的**硬限制**。任何父元素的 `overflow: hidden` 都会裁剪 `<select>` 弹出层、自定义下拉框等。

**解决方案优先级**：
1. 改用自定义下拉组件（`.custom-select`，用 `position: absolute; z-index: 999` 脱离 overflow 上下文）
2. 或：在打开弹窗时动态添加 class 解除 overflow
3. **不要**依赖 inline style（在 WebView2 中不稳定）

```css
/* 动态解除 overflow 的 pattern */
.plugin-open {
  overflow: visible !important;
  max-height: none !important;
}
```

> **⚠️ 只对本条描述的「纯裁剪容器」生效（2026-09-19 补 —— 缺这一句就是事故）**
>
> `.plugin-open` 把 `overflow` 改成 `visible`，对**做裁剪用**的容器是对的；但加到一个**靠自身滚动**的列表上（典型：`#results-list`、`.tool-result`、`#chat-log` 的子滚动盒），会直接把它的滚动盒拆掉 —— 表现是 `scrollTop` 归零、整页跳回顶部、再也滚不动。判断口径只有一条：**这个元素是不是滚动条真正出现的那一层？** 是 → 不能用这个 pattern，改用「把弹窗挂到 `body` 下」或「换自定义下拉组件」。
>
> 依据：`docs/agent-ui-spec.md` §5.4 第 3 条「不要『藏』滚动条」——`overflow: visible` 与 `overflow: hidden` 在「把滚动盒拆掉」这件事上是同一个后果。

### 5.2 原生 `<select>` 在 WebView2 透明窗口中不可用

`tauri.conf.json` 中 `"transparent": true` 时，WebView2 不会渲染原生 `<select>` 弹出层。**必须用自定义下拉组件替代。**

### 5.3 `pointer-events: none` 的区域无法点击

```css
#app { pointer-events: none; }  /* 整个窗口穿透 */
#search-bar { pointer-events: auto; }  /* 逐一恢复 */
#results-container { pointer-events: auto; }
#status-bar { pointer-events: auto; }
```

- 所有交互子元素必须显式 `pointer-events: auto`
- 窗口边缘 padding 会制造不可交互死区 — 用子元素 margin 替代

### 5.4 自定义滚动条：**统一细滚动条，不得按容器声明、不得隐藏**

> **本节 2026-09-19 重写。** 旧版本教的是下面这段，**它是被明令禁止的写法，照抄会直接制造事故**：
>
> ```css
> /* ❌ 禁止。历史事故就是这一段被复制到新容器上 */
> .custom-select-dropdown { scrollbar-width: none; }
> .custom-select-dropdown::-webkit-scrollbar { display: none; }
> ```
>
> 两个错误：① `scrollbar-width: none` 一出现，Chromium 就**忽略全部 `::-webkit-scrollbar`**（滚动条退回系统默认外观）；② 「藏滚动条」与项目要求相反。

**唯一真相源是 [agent-ui-spec.md](./agent-ui-spec.md) §5.4**（三条硬约束：禁止写 `scrollbar-width` / `scrollbar-color`；禁止按容器单独声明；不要「藏」滚动条）。实现只有一处 —— `styles.css` 的全局 `::-webkit-scrollbar` 规则；新容器**只要写 `overflow-y: auto` 就自动获得统一外观，不需要任何额外 CSS**。

对应 [ai-spec.md](./ai-spec.md) §11 规则 21 的「不得回退」项。

### 5.5 隐藏元素用 class 控制，避免 display 冲突

```css
/* ❌ 两个规则冲突 */
.hidden { display: none !important; }
#chat-drawer.visible { display: block; }

/* ✅ 用 pointer-events + transitionend 延迟 hidden */
#chat-drawer { pointer-events: none; opacity: 0; transition: opacity 0.3s; }
#chat-drawer.visible { pointer-events: auto; opacity: 1; }
/* transitionend 后再加 .hidden */
```

### 5.6 HTML 元素 class 操作优于 inline style

```typescript
// ❌ inline style 可能被 CSS 覆盖
resultsContainer.style.overflow = "visible";

// ✅ 用 class + !important
resultsContainer.classList.add("plugin-open");
```

---

## 第 6 章：DOM / 状态管理规则

### 6.1 `innerHTML` 序列化不保留 DOM property

```html
<!-- 用户点击 checkbox → DOM property checked=true -->
<!-- 但 innerHTML 序列化时 checked 属性不出现 -->
<input type="checkbox">  <!-- 永远是未选中 -->
```

**规则**：任何依赖用户交互状态（checkbox/select/textarea）的插件，**不能用缓存 HTML 恢复**。必须跳过缓存、重新执行 `plugin.execute()`。

### 6.2 批量 DOM 操作用 DocumentFragment

```typescript
// ❌ 逐条插入 → 多次重绘
results.forEach(item => resultsList.appendChild(item));

// ✅ 批量替换 → 一次重绘
const frag = document.createDocumentFragment();
results.forEach(item => frag.appendChild(item));
resultsList.replaceChildren(frag);
```

### 6.3 流式传输中途关闭 → 递增 streamId 防回调污染

```typescript
let streamId = 0;

function startStream() {
  const myStreamId = ++streamId;
  // 回调中检查：
  streamCallback = (data) => {
    if (myStreamId !== streamId) return; // 已关闭，忽略
    // ... 处理数据
  };
}
```

### 6.4 关闭组件时必须完整清理状态

```typescript
function closePluginView() {
  // 必须清理的全部状态：
  if (isStreaming) {
    streamId++;                    // 防回调污染
    isStreaming = false;
    setStreamingUI(false);         // 恢复发送/停止按钮
    streamCallback = null;
    doneCallback = null;
    cliTextCallback = null;
    cliDoneCallback = null;
  }
  agentView = null;               // 清 agent 渲染状态
  currentSessionId = null;        // 下次保存用新 ID
  consecutiveFailures = 0;        // 重置错误计数
  pendingMessages = [];           // 清消息队列
  humanizeBtn?.classList.add("hidden");
}
```

### 6.5 渲染函数必须守卫 `pluginActive`

```typescript
function renderMixedResults(...) {
  if (pluginActive) return; // 插件面板打开时不覆盖其 HTML
  // ...
}
```

---

## 第 7 章：热键系统规则

### 7.1 双后端 + 三层兜底架构（2026-09 修订）

**后端由 `install_hook_thread` 决定，优先级不可颠倒**：先 `RegisterHotKey`，成功则不装钩子；失败才 `SetWindowsHookEx(WH_KEYBOARD_LL)` 兜底。

| 层级 | 覆盖场景 | 实现位置 |
|------|---------|---------|
| **RegisterHotKey**（默认，无钩子） | 其他应用/管理员游戏焦点 | `hotkey.rs` |
| **WH_KEYBOARD_LL**（仅注册失败时） | `Alt+Space` 系统保留 / 组合被占用 | `hotkey.rs` |
| **WndProc 子类化** | 自身窗口焦点（系统菜单路径） | `hotkey.rs` |
| **JS keydown 兜底** | 漏网按键（仅 Alt+Space 配置时） | `main.ts` |

**为何优先 RegisterHotKey**：LL 钩子 + `SendInput` 注入是杀软判定的键盘记录器特征（本项目曾因此被 Defender 报 `Trojan:Win32/Prowloc.A!cl` 误报）；且 LL 钩子受 UIPI 限制（游戏以管理员运行时收不到按键）。`RegisterHotKey` 无钩子、无注入，且不受前台程序权限影响。

**铁律**：
- 不要为了「少写分支」而无条件装钩子——会同时引入 AV 误报与游戏失效两个回归。
- `SendInput` dummy key **仅限 LL 钩子分支**（RegisterHotKey 由系统消费按键，无孤立 Alt 序列，注入纯属多余且增加 AV 特征）。
- 看门狗健康检查必须用 `HOOK_MODE` 守卫，否则无钩子模式会被误判为「钩子死亡」而反复重装。
- `install_hook_thread` 重装前必须向旧消息泵线程 `WM_QUIT`，否则改键会累积多个泵 → 热键与 Esc 重复响应。
- **热键判定必须「主键 + 修饰键」同时比对**：`VK_SPACE` 是 `Alt+Space` 与 `Ctrl+Alt+Space` 共用的主键，只比 VK 会让旧组合继续生效（曾出现「无论怎么改键，Alt+Space 都能呼出」）。任何新增的热键比较分支都要照此处理。
- **`HOTKEY_MODIFIERS` / `HOTKEY_VK` / `HOTKEY_COMBO` 必须经 `parse_and_set_hotkey` 一起设置**，且静态初值要与 `DEFAULT_HOTKEY` 一致。只改其中一部分 → 界面显示与实际按键不符（显示 Ctrl+Alt+Space、实际按 Alt+Space），并会连带把后端误判为「注册失败」而装钩子。

### 7.2 LL 钩子回调铁律

- **回调内只能 `PostThreadMessageW`**（微秒级返回），不能调用 Tauri 窗口 API
- **窗口操作丢独立线程**执行
- **Alt 检测必须用无状态查询**：`LLKHF_ALTDOWN` flag + `GetAsyncKeyState`
- **禁止手动 down/up 跟踪 Alt**：UAC/管理员窗口吞 Alt up → 永久卡死
- **Alt 的 vkCode**：`VK_LMENU(0xA4)` / `VK_RMENU(0xA5)`，不是 `VK_MENU(0x12)`

### 7.3 WndProc 子类化规则

- `WM_SYSCOMMAND/SC_KEYMENU` 是系统菜单弹出的**唯一**消息路径
- `lParam=0x20` 为 Alt+Space
- 录制期间不可吞 Alt+key → 检查 `RECORDING` 原子变量
- 非录制期间 `WM_SYSCOMMAND/SC_KEYMENU` **一律 `return 0`**

### 7.4 JS 兜底规则

- 仅当 `window.__lunac_hotkey_is_alt_space === true` 时拦截 Alt+Space
- 自定义热键后该标志为 `false` → JS 完全不干预
- 录制 Alt+Space 时 Chromium 吞 Space keydown → 只能走 WndProc 子类化路径（`RECORDING=true` 时 emit `lunac-hotkey-recorded`）

### 7.5 热键持久化环节

- `lunac-hotkey-recorded` listener：更新按钮文字 **+** 调用 `invoke("set_hotkey_combo")`
- `parse_and_set_hotkey` 末尾：重装热键后端（`install_hook_thread` → 优先 RegisterHotKey，失败才装钩子）；主窗口句柄未就绪时只设原子变量，由 `start_hotkey` 末尾统一装配
- `status.hotkey_hint` 用 `{hotkey}` 插值，改键后经 `__lunac_refresh_hotkey_hint` 刷新状态栏（避免残留旧热键）
- 启动时 `repair_auto_start_on_startup` 检查注册表路径一致性

---

## 第 8 章：窗口管理规则

### 8.1 前台锁定（ForegroundLockTimeout）

Windows 禁止后台进程抢占前台。Tauri `set_focus()` 可能静默失败。

**修复链**：`AttachThreadInput` 附加前台线程 → `SetForegroundWindow` 夺焦 → 3 次重试（间隔 16ms）

### 8.2 最小化检测

```rust
// IsWindowVisible 在最小化时仍返回 true
// 必须加 IsIconic 检测
if IsIconic(hwnd) {
    // 走 SW_RESTORE 而非 toggle
}
```

### 8.3 窗口 resize 规则

- `resizable: false` + `transparent: true` + `decorations: false` → **移除 `minWidth/minHeight`**（与 Tauri 2 有交互 bug）
- `setSize` 需 `core:window:allow-set-size` 权限
- WebView2 视口不能渲染超出物理窗口尺寸的内容 → 不能用 JS 测量再 resize 的内容驱动方案

### 8.4 隐藏/显示规则

```typescript
// 搜索内容不随失焦清空
win.hide();  // 纯隐藏，不清空任何状态
// Esc 才重置搜索框内容
```

- `onFocusChanged(false)` → 150ms debounce 防 WebView2 渲染瞬发
- 程序化 hide 时设 `blockingHide = true` 防 `onFocusChanged` 重复触发

---

## 第 9 章：流式传输 / AI 对话规则

### 9.1 streamId 守卫

```typescript
let streamId = 0;

// 任何会销毁正在流式渲染的 DOM 的操作前：
streamId++;  // 旧回调全部失效
isStreaming = false;
setStreamingUI(false);
streamCallback = null;
doneCallback = null;
cliTextCallback = null;
cliDoneCallback = null;
```

**触发场景**：`restoreSession`、`newConversation`、`closePluginView`、`startAgentChat` 错误路径、`startAIChat` 错误路径

### 9.2 消息队列模式

```typescript
let pendingMessages: string[] = [];
// 流式中用户发送消息 → 入队
// doneCallback 末尾 processQueue() 依次发送
```

### 9.3 会话保存用 id 复用

```typescript
let currentSessionId: string | null = null;
// 首次保存生成 ID，后续同一对话复用同一 ID 实现 upsert
// 避免每次保存都创建新记录
```

### 9.4 抽屉 DOM 清理

```typescript
// 关闭抽屉：直接 remove backdrop，不要仅 opacity:0
drawerBackdrop.remove();
drawerBackdrop = null;
```

---

## 第 10 章：剪贴板规则

### 10.1 双路径归一化

Rust 侧 `text.trim()` + JS 侧 `readText()` 返回的文本格式不一致（尾部换行/空格）。

```typescript
// ✅ 入口统一归一化
const normalized = text.replace(/\r\n/g, '\n').trim();
```

### 10.2 搜索框去重

```typescript
// 检查当前 searchInput.value，不只是历史记录
if (firstPart === searchInput.value.trim()) {
  return; // 搜索框已存在，跳过
}
```

### 10.3 剪贴板读取线程安全

读取剪贴板**必须用 `app.run_on_main_thread()`**，不能在线程池线程中读取。

---

## 第 11 章：常见反模式速查表

| 反模式 | 后果 | 正确做法 |
|--------|------|---------|
| 新增 API 不声明权限 | 调用静默失败 | 先加 `capabilities/default.json` |
| `invoke` 参数名与 Rust 签名不一致 | 保存静默失败 | 逐参数对齐 Rust `#[tauri::command] fn` 签名 |
| 串行 `await` 隐藏+启动 | 用户感知延迟 | `win.hide()` + `invoke().catch()` 无 await 并发 |
| 依赖 `innerHTML` 恢复 checkbox 状态 | checkbox 永远未选中 | settings 插件跳过缓存 |
| `scrollHeight` 测量决定窗口尺寸 | WebView2 视口裁剪导致测量不准 | 用确定性尺寸（条目数映射/固定高度） |
| `document.createElement` + `appendChild` 逐条插入 | 多次重绘闪烁 | `DocumentFragment` + `replaceChildren` 批量替换 |
| 流式回调无 streamId 守卫 | 关闭后旧数据污染新对话 DOM | `myStreamId = ++streamId` + 回调中 `if (myStreamId !== streamId) return` |
| CSS `overflow: hidden` + `<select>` 弹窗 | 下拉框无法展开 | 自定义下拉组件 |
| inline style 解除 overflow | WebView2 中不稳定 | 用 class + `!important` |
| transition 后无 fallback timeout | transitionend 永不触发 | `setTimeout(fallback, 500)` |
| 静态 import 改动态 import | listener 未就绪 | 保留静态 import，必要时 await 确保就绪 |
| `opacity:0` 隐藏 overlay | overlay 仍占 DOM 拦截点击 | `element.remove()` |
| 不检查 `pluginActive` 渲染混合结果 | 搜索渲染覆盖插件面板 | `if (pluginActive) return` |
| 不清理 `pendingMessages` | 新对话自动发送旧消息 | `closePluginView` 中 `pendingMessages = []` |

---
## 第 12 章：检查清单（PR/MR 前必过）

- [ ] `capabilities/default.json`：所有新增 API 调用均已显式授权
- [ ] `invoke()` 参数名与 Rust `#[tauri::command] fn` 签名一致
- [ ] 无串行 `await` 导致的延迟（隐藏、启动、读写并发优先）
- [ ] 新增渲染路径有关闭清理逻辑（streamId / isStreaming / callbacks）
- [ ] 无依赖 `innerHTML` 恢复交互状态的代码
- [ ] 无 CSS `overflow: hidden` 裁剪弹窗的风险
- [ ] 无原生 `<select>` 依赖（透明窗口下不渲染）
- [ ] `renderMixedResults` 等渲染函数有 `pluginActive` 守卫
- [ ] 剪贴板文本处理有归一化步骤
- [ ] 热键修改兼顾 RegisterHotKey + LL 钩子 + WndProc + JS 四层（不可无条件装钩子）
- [ ] 新增 UI 字符串使用 `t("key")` 而非硬编码文本（包括英文）
- [ ] 新增 `t()` key 需要同时在 `i18n.ts` 的 DICT 中添加翻译（至少 zh-CN, en）
- [ ] 新增按钮 / 控件已声明主题样式（`border-style` 不得为 `outset`；同族按钮并入同一族规则 —— ai-spec §11 规则 51）
- [ ] 改动过 `history` 装配的，已按 ai-spec §11 规则 23 核过 agent 日志的 `公共前缀=N/M条`（出现 `N < M` 即本侧就地改写）与 `请求用量` 行的 `read`（是否塌到 `system+tools` 量级）
- [ ] 新增 / 改名内置工具的，已同步 `defs()` + 三张判断表 + ai-spec §3.5 契约表 + `agent-implementation.md` §4.1 总数口径 + 前端黑名单候选名单，且 `cargo test` 的工具总数守门单测通过（ai-spec §11 规则 14 / 54）
- [ ] `npx tsc --noEmit` 通过
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过（`core-agent` 与 `src-tauri` 两侧）

---
## 第 13 章：国际化 (i18n) 规则

### 13.1 使用 `t()` 而非硬编码文本

```typescript
// ❌ 硬编码（只显示一种语言）
statusText.textContent = "AI thinking...";
chatInput.placeholder = "Ask anything...";

// ✅ 使用 t() 动态翻译
statusText.textContent = t("chat.ai_thinking");
chatInput.placeholder = t("chat.placeholder");
```

### 13.2 参数插值

```typescript
// ✅ t() 支持 {param} 占位符
t("status.running_plugin", { name: pluginName(plugin.id) });
t("agent.done", { count: String(turn.toolCalls.length) });
```

### 13.3 新增 key 必须同时加翻译

任何 `t("new_key")` 调用必须在 `app/src/i18n.ts` 的 `DICT` 对象中添加对应条目，至少包含 `zh-CN` 和 `en` 两种语言。

### 13.4 语言生命周期

```typescript
// 启动时：先检测系统语言，再加载用户偏好覆盖
await initI18n();       // Rust GetUserDefaultUILanguage → BCP-47
loadSavedLanguage();    // localStorage 覆盖（用户手动选择）
// 运行时切换：
setLanguage("ja");      // 切换 + 持久化
```

---
*最后更新：2026-09-19 · 来源：项目 100+ 条 bug 修复经验 + 5 天高强度迭代 · 关联文档：[ai-spec.md](./ai-spec.md)（规范正文）、[agent-ui-spec.md](./agent-ui-spec.md)（界面/滚动条的唯一真相源）*

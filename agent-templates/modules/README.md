# Lunac 插件（Modules）开发规范

> 这份文档既是**给人看的**说明书，也是**给 Lunac 自己的 AI 看的**规范 ——
> 它被放在 `<exe 根>\Modules\README.md`，agent 的系统提示词里给了这个绝对路径。
> 想让 Lunac 帮你写一个插件，直接说「照 Modules\README.md 的规范给我做一个 XXX 插件」即可，
> **不需要 dev 版、也不需要任何前端构建环境**：插件就是几个纯文本文件。
> （只有「自带 UI 框架 / 用 TypeScript」这类进阶玩法才需要你自己的打包步骤，见 4.4 与第 6 节。）

---

## 1. 插件是什么

Lunac 的搜索结果里，除了内置功能，还能出现**插件**。
插件 = 一个文件夹 + 一份清单 + 一个 ESM 入口。装上之后它在同一个搜索框里被搜到、点开即在结果区渲染。

**一切都以磁盘为准**：`<exe 根>\Modules\<插件 id>\` 这个目录存在、清单与入口都合法，
插件就存在。没有注册表、没有中心数据库 —— 目录就是唯一真相源（卸载 = 删目录）。

### 基础插件 vs 拓展插件（2026-09-29 起）

| 分类 | 名单 | 随安装包 | 能装 / 能卸 | 代码在哪 |
|---|---|---|---|---|
| **基础** | 快速启动 / 设置 / 网页搜索 / AI 助手 / 备忘录 / 翻译 | ✅ 一定装 | ❌ 都不行 | 编译进主程序 |
| **拓展** | 剪贴板历史 / OCR / 音乐歌词 / 文件转换，**以及今后新增的一切插件** | ❌ 默认不装（安装包里可勾选） | ✅ 市场下载 / 卸载 | `Modules\<id>\`（就是本目录里的东西） |

> 翻译是 2026-09-29 被**点名**加进基础插件的（它的联网与模型调用都在主程序里，
> 见 `docs/ai-spec.md` §4.9）。「今后新增的一切都算拓展」这条规矩不变 ——
> 那是默认值，只有明确指定的才例外。

**所以：你要做的插件都算「拓展插件」**，放进本目录即可，不需要、也不可能改主程序。

### 「一条引用路径」：不需要为插件改任何宿主代码

宿主**不认识任何具体插件**。它在**启动时**、**每次点「重新扫描」时**，
以及**AI 在对话里往本目录写了文件的那个回合结束时**（自动重扫，见第 8 节第 5 步）扫一遍本目录，
把合法的插件逐个 load 起来 —— 这条「目录扫描」就是**唯一那一条中转引用**：

```text
主程序 → 扫描 <exe 根>\Modules\*  → 每个 <id>\index.js
```

所以新增插件**永远**只是「把文件夹放进本目录」，与插件数量无关；
插件**卸载**也就是删掉那个目录，主程序里不会留下任何指向它的引用
（「卸载后完全不存在于本应用」是硬要求）。

```text
<exe 根>\Modules\
  README.md              ← 这份规范（不是插件，宿主会忽略非目录条目）
  <插件 id>\
    lunac-plugin.json    ← 清单（必需）
    index.js             ← 入口，已编译好的 ESM（必需，文件名由清单的 entry 指定）
    <其它文件>            ← 图片 / 数据 / 依赖，随便放，用相对路径引用
```

## 2. 清单一：`lunac-plugin.json`

```json
{
  "id": "pomodoro",
  "name": "番茄钟",
  "description": "25 分钟专注计时器",
  "keywords": ["pomodoro", "番茄", "专注", "计时"],
  "icon": "🍅",
  "version": "1.0.0",
  "entry": "index.js",
  "homepage": "https://example.com/pomodoro",
  "dependencies": []
}
```

| 字段 | 必需 | 说明 |
|---|---|---|
| `id` | ✅ | **同时是目录名**。只允许小写字母、数字、`-`、`_`、`.`；不能以 `.` 开头；≤ 48 字符 |
| `entry` | | ESM 入口，**包内相对路径**、以 `.js` / `.mjs` 结尾。缺省 `index.js` |
| `name` | | 显示名（中文会走拼音匹配，所以中文名照样能被 `fq` 这种拼音搜到） |
| `description` | | 一句话说明 |
| `keywords` | | 搜索关键词数组。**至少给几个** —— 搜索全靠名字 + 关键词 |
| `icon` | | emoji 兜底图标（可留空） |
| `version` | | 版本号字符串，插件市场用它判断「有没有新版」 |
| `homepage` | | 作者 / 仓库地址（界面显示来源用） |
| `dependencies` | | 依赖数组，见第 5 节。没有就写 `[]` 或省略 |
| `permissions` | | **声明的宿主能力**，见下面「宿主能力」一段。没有就写 `[]` 或省略 |
| `reuse` | | **复用哪个内置（基础）插件的挂载监听**，见下面「复用内置插件监听」一段。没有就写 `[]` 或省略 |
| `window` | | **悬浮窗形态参数**（宽高 / 能不能缩放 / 进不进任务栏 / 要不要标题栏），见下面「窗口形态」一段。没有就省略 |
| `sidecar` | | **插件自带的本机进程**（可执行文件 / 参数 / 通道），见下面「自带本机程序」一段。**必须同时声明 `permissions: ["process.spawn"]`**。没有就省略 |

### 宿主能力：`permissions`

有些插件要动**窗口本身**，不能只往结果区里画东西。这类需求必须**在清单里声明**，
宿主才会照做（并且会在插件市场上如实把它列给用户看）：

```json
"permissions": ["window.float", "layout.takeover"]
```

| 能力名 | 效果 |
|---|---|
| `window.float` | 这个插件一律在**独立悬浮窗**里打开（而不是结果区里那一条） |
| `layout.takeover` | **接管整个窗口**：结果 HTML 直接铺满并切成 detached 模式，`attach(root)` 拿到的 `root` 是文档根（此时节点 id 要在整篇文档里唯一，用 `document.getElementById` 找即可） |
| `window.resize` | 允许插件请求宿主改变窗口尺寸 |
| `process.spawn` | **允许插件起一个自带的本机进程**（配合清单 `sidecar` 段，见下面「自带本机程序」）。**与其它能力不同：这一条必须过信任卡**（默认拒绝）—— 能 spawn 任意二进制的插件能力无上界，不能靠「声明即生效」 |

> 诚实说明：这一层是**声明 + 告知**，不是沙箱 —— 插件是本机可执行代码。
> 没有声明的能力不会生效（而且是静默不生效），所以**要什么就写什么**。
> 写错名字（宿主不认识）只会被记一条警告，不影响安装。

### 复用内置插件监听：`reuse`

**问题**：内置插件（快速启动 / 设置 / 备忘录 / 翻译 / 工具编辑器…）的面板 HTML 里的按钮、输入框，
靠的是**编译进主程序的那套监听**（`attachXxxListeners`）。如果你只是把它们的面板 HTML 抄进
自己的插件，那些按钮**全是死的** —— 因为磁盘插件默认拿不到宿主的监听。

**解法**：在清单里声明 `reuse`，宿主会在调用**你自己的** `attach(root)` 之外，
**再把被复用插件的监听也挂到同一个 `root` 上**：

```json
"reuse": ["settings"]
```

| 可复用的 id | 对应面板 |
|---|---|
| `settings` | 设置面板（[builtin/settings.ts](../../app/src/plugins/builtin/settings.ts)） |
| `quick-launch` | 快速启动 |
| `memo` | 备忘录 |
| `translate` | 翻译 |
| `tool-editor` | 工具编辑器 |

三条纪律：

1. **只认「基础插件」**（上表这几个）。拓展插件（`Modules\` 下的磁盘插件）**不能**被 reuse ——
   它本来就有自己的 `attach`，宿主直接调它即可。写了个不存在的 id 只会记一条警告、不影响安装。
2. **最多 4 条**（`MAX_REUSE`），条目也要合法（`[a-z0-9._-]`），否则清单校验不过。
3. **`reuse` 不是「变成那个插件」** —— 搜索结果显示的仍是你自己的名字 / 图标 / 关键词，
   只是结果区挂载时多挂了一套内置监听。你仍要自己实现 `execute(input)` 产出与那些监听
   匹配的 HTML（节点 id / class 必须与内置面板一致，监听才找得到元素）。

> 什么时候用它：你想「照抄设置面板的样式和交互做一个变体」时。纯自绘面板不需要它。

### 窗口形态：`window`

`permissions` 回答的是「**这个插件被允许动什么**」，这一段回答的是「**它的悬浮窗长什么样**」——
两句不重叠，所以它是**单独的清单字段**，不是一条能力名。**只有该插件本来就会开独立窗时它才起作用**
（「能不能开独立窗」仍然只认 `window.float`）。

```json
"window": {
  "width": 300, "height": 400,
  "minWidth": 160, "minHeight": 200,
  "resizable": false,
  "skipTaskbar": true,
  "alwaysOnTop": true,
  "chrome": false
}
```

| 字段 | 缺省 | 说明 |
|---|---|---|
| `width` / `height` | 宿主缺省（420×560） | 初始尺寸（逻辑像素）。写 `0` 表示「用宿主那一档」 |
| `minWidth` / `minHeight` | 宿主缺省（300×200） | 最小尺寸。同样可以写 `0` |
| `resizable` | `true` | 允不允许用户手动拉边框。定尺面板写 `false` |
| `skipTaskbar` | `false` | 进不进任务栏。桌宠这类常驻小窗建议 `true` |
| `alwaysOnTop` | `true` | 建窗即置顶 |
| `chrome` | `true` | 要不要宿主那根标题栏（置顶 / 最小化 / 关闭三个按钮）。写 `false` 时**整窗连结果区的玻璃底一起去掉**，只剩你画的内容 —— 适合「窗口本身就是那片画面」的插件 |

三条注意：

1. **不写这一段 = 与加它之前逐项一致**（420×560、可缩放、进任务栏、有标题栏）。
   没有形态要求的插件**不要**补一个空对象，那不是必需的。
2. 宿主**建窗时**读它（读的是这份清单），所以改完要**重新扫描 / 重启**才生效；
   已经开着的那个窗不会被改形态。
3. `chrome: false` 时那三个按钮不存在 —— **关闭 / 最小化要你自己提供入口**
   （右键菜单、面板上的按钮都行）。

### 自带本机程序：`sidecar`（进阶，2026-10-06）

WebView 里做不了的事（跑 `ffmpeg` / `aria2` / `llama.cpp`、开串口 / USB、用别的语言写的引擎），
可以**带一个自己的二进制**，由宿主负责**起进程 + 通信 + 收尾**。UI 仍在 WebView 里（不替代 WebView）。

```json
{
  "permissions": ["process.spawn"],
  "sidecar": {
    "command": "bin/mytool.exe",
    "args": ["--stdio"],
    "transport": "stdio",
    "autostart": false,
    "restart": "on-failure",
    "sha256": "9f2c…"
  }
}
```

| 字段 | 说明 |
|---|---|
| `command` | **必需**。可执行文件**相对插件目录**的路径（不许绝对路径 / 不许含 `..`）。可与 `dependencies[{type:"file"}]` 配合先下载、再起 |
| `args` | 参数数组（**不经 shell**，直接传给进程 —— 别想在这拼命令行） |
| `transport` | `stdio`（缺省）/ `http` / `both`，见下 |
| `port` | `http` / `both` 时用；`0`（缺省）= 进程自选并经**握手行**上报 |
| `cwd` | 工作目录（相对插件目录，**不得越界**）。缺省 = 插件目录 |
| `env` | 附加环境变量（在宿主环境之上**只增不删**） |
| `autostart` | 面板打开时自动起（缺省 `false` = 你在 `attach` 里调 `start()`） |
| `restart` | 崩溃重启：`never`（缺省）/ `on-failure`（最多 3 次） |
| `sha256` | 可执行文件校验和（64 位十六进制，**建议写**；写了就必须对上） |
| `allowLocalPorts` / `allowLocalAny` | **额外授权**访问的本机端口，见下 |

> **同一段 `sidecar` 里没有 `permissions: ["process.spawn"]` 会被直接拒装** —— 两者必须同时出现。

**信任门（硬）**：第一次 `start()` 会弹一张卡，如实列出 `command` / `args` / `sha256` /
可访问端口，由**用户**裁决（仅本次 / 始终信任 / 拒绝）。**默认拒绝** —— 没批准绝不起进程。
改了 `command` 或 `sha256` 会让旧信任**失效**、重新弹卡。

**怎么用（插件侧都走同一个桥）**：

```js
const sc = globalThis.__lunac_host?.sidecar;   // apiVersion >= 2 才有；旧宿主上务必 ?. 兜底
if (!sc) { /* 旧宿主：降级或提示 */ }

await sc.start();                               // 起进程（首次会先弹信任卡，是异步的）
const r = await sc.request("ping", { n: 1 });   // stdio：NDJSON 请求 → 回包
const off = sc.on("progress", p => console.log(p)); // 订阅进程主动推
await sc.stop();                                // 收进程（面板关闭时宿主也会自动收）
```

- **`stdio`**：宿主 ↔ 进程走 stdin/stdout 的 **NDJSON**（每行一个 JSON 对象）。进程**必须先发一行**
  `{"method":"ready"}`（`http` 时带 `{"params":{"port":12345}}`），宿主收到才算起好了（超时 10s）。
- **`http`**：进程在 `127.0.0.1:<port>` 上监听。**插件自己不能 `fetch()` 这个端口**（宿主 CSP 拦得住），
  必须走**宿主代请求**：`await sc.http({ method: "POST", path: "/v1/chat", body: "…" })` → `{ status, body }`。

**访问范围默认只有它自己的端口**。要访问**别的**本机应用（例如本机大模型 `http://127.0.0.1:11434`），
必须在清单里声明 `allowLocalPorts: [11434]` 或 `allowLocalAny: true` —— 信任卡会把它**显著列出来**，
**由用户批准**。**AI 不能自己把门推开**：它只能写这份声明 / 拉起这张卡。

**三条纪律**：

1. **必须绑定面板**：面板一关，宿主就收掉进程（**不许「看不见的常驻进程」**）。
   所以别在 `execute()` 里同步起进程 —— 先画面板，在 `attach(root)` 里再 `start()`。
2. **收尾归你**：进程是**完整本机程序**，不是沙箱。声明什么才有什么，别声明用不到的。
3. **进程要自己退**：宿主会 `kill`，但你的子进程如果又派了孙子进程，得自己在退出路径上收干净。

## 3. 入口契约：`index.js`

入口是一份**原生 ESM**（`import` / `export`，不是 CommonJS 的 `require`）。至少要有一个执行函数：

```js
// 最简形态：默认导出一个对象
export default {
  async execute(input) {
    return `你输入的是：${input}`;
  },
};
```

三种写法都被接受（选一种即可）：

```js
export default async function (input) { return "…"; }        // 默认导出就是函数
export default { async execute(input) { return "…"; } };      // 默认导出对象里的 execute
export async function execute(input) { return "…"; }          // 具名导出
```

**`execute(input)` 的入参**是用户在搜索栏里输入的原始文本（与内置插件一致）。
**返回值**只能两种形状：

| 返回 | 结果区渲染成 |
|---|---|
| 字符串 `"…"` | 纯文本（会被转义，安全） |
| `{ type: "text", content: "…" }` | 同上 |
| `{ type: "html", content: "<div>…</div>" }` | **原始 HTML**（用 `innerHTML` 插入，见第 7 节的安全说明） |

返回别的东西 ⇒ 界面显示一条明确错误（不会静默变空块）。

### 可选：`attach(root)` / `detach()`

如果插件结果里**有交互控件**（按钮、输入框、定时器、订阅事件），就再导出两个钩子 ——
结果 HTML 插进 DOM 之后，宿主会调 `attach(root)`，其中 `root` 是结果区容器：

```js
let timer = 0;

export default {
  async execute(input) {
    return { type: "html", content: `<button id="pomo-go">开始</button><span id="pomo-t"></span>` };
  },
  attach(root) {
    const btn = root.querySelector("#pomo-go");
    btn?.addEventListener("click", () => {
      let left = 25 * 60;
      timer = setInterval(() => {
        const el = root.querySelector("#pomo-t");
        if (!el) return;                       // 面板已被换掉 ⇒ 自己停
        el.textContent = String(--left);
        if (left <= 0) clearInterval(timer);
      }, 1000);
    });
  },
  detach() { clearInterval(timer); },          // 面板关闭时收尾
};
```

两条纪律：

1. **自己开的定时器 / 监听，自己在 `detach()` 里收掉**；同时留一句
   `if (!root.isConnected) return;` 之类的自停兜底（不依赖宿主一定调到 `detach`）。
2. `attach` 里一律用 `root.querySelector(...)` 找节点，**不要用全局 `document`** ——
   同一个插件可能同时开在结果区和独立悬浮窗里。

不需要交互的插件**不用写**这两个钩子（写完就返回文本最省事）。

## 4. 能用到什么

### 4.1 宿主桥 `__lunac_host`

插件是独立打包的模块，**不能** `import` 宿主的内部模块（那会拿到另一份未初始化的副本）。
需要宿主能力时走桥：

```js
const t = (key, params) => globalThis.__lunac_host?.t?.(key, params) ?? key;

t("music.play");   // → 宿主当前语言的翻译
```

| 桥字段 | 说明 |
|---|---|
| `t(key, params?)` | 宿主 i18n 的翻译函数（与内置插件共用同一份语言状态） |
| `apiVersion` | 桥的协议版本，当前 `1`。可据此做兼容分支 |

> 桥是**可选**的：插件完全可以用自己写死的文案（多语言插件再自己判断 `navigator.language`）。
> **发现 `undefined` 时务必给兜底**（`??`），否则在旧版宿主上会整个插件报错。
>
> ⚠️ **宿主只认识它自己那本词典里的 key**。你给插件起的新 key（比如 `t("pomodoro.go")`）
> 在宿主词典里不存在 ⇒ 界面上会**原样显示 `pomodoro.go`**（`t()` 找不到就返回 key 本身）。
> 所以自己写的插件请**直接把文案写死**，别指望 `t()` 能翻译你的词；
> 要支持多语言就自己做个 `const S = { "zh-CN": {...}, en: {...} }` 按语言取。

### 4.2 Tauri API

`@tauri-apps/api` 那一套（`invoke` 调宿主命令、`listen` 订阅事件）可以直接用，
把它的源码**打进你的插件包**即可（见第 6 节的打包说明）。它读的是窗口上的
`window.__TAURI_INTERNALS__`，在任何 Lunac 窗口里都在。

### 4.3 样式

宿主的结果区会自带主题变量（`--text` / `--text-dim` / `--bg` / `--border-glass` /
`--accent` 等）。**用这些变量写内联样式**，插件才会跟着用户换主题：

```js
content: `<div style="color:var(--text-dim);padding:8px">…</div>`
```

插件自己的 `<style>` 也可以插在结果 HTML 里（记得给类名加插件前缀，避免和宿主撞车）。

### 4.4 自带 UI 框架 / 第三方库（**允许**）

宿主自己**不引** UI 框架与图标库（那是 `agent-ui-spec.md` §0 给**宿主面板**定的硬边界），
但那条纪律**只管宿主自己**，**管不到插件** —— 插件自带框架是被明确允许的：
WebView2 就是个 Chromium，React / Vue / Svelte / Solid / lit / petite-vue 都能跑。

两条路（**优先第一条**）：

| 路径 | 做法 | 代价 |
|---|---|---|
| **打进入口（推荐）** | `import` 后由打包器把框架代码**内联进 `index.js`**（单文件约束见第 6 节） | 产物变大（React ≈ +150KB）；换来**零外部依赖**、任何机器都能跑 |
| `npm` 依赖 | 清单写 `dependencies[{type:"npm", …}]`，装进插件目录的 `node_modules\` | **要求用户机器有 Node.js**；升级 / 离线机器更易坏 |

四条注意：

1. **必须和宿主主题变量对齐**：框架自绘的组件默认不认 `--text` / `--bg` / `--accent`（见 4.3），
   明暗主题下容易和宿主割裂 —— 把框架的主题 token 接到宿主变量上，或至少留一个跟随
   `prefers-color-scheme` 的兜底。
2. **`type:"html"` 是 `innerHTML` 插入**：框架要在 `attach(root)` 里 **mount 到 `root`**（别用
   `document.body`），并在 `detach()` 里 **unmount** —— 否则面板关闭后框架的监听 / 定时器会留在后台。
3. **仍然禁止 `import()` 网络地址**（CSP，见第 6 / 7 节）：框架必须**打进入口文件**，不能从 CDN 拉。
4. **多插件之间不会自动去重**：宿主不提供共享运行时，每个插件各自 bundle 一份；在意体积就用轻量方案
   （petite-vue / lit / 原生 Web Components）。

> 一句话：**插件可以比宿主「豪华」** —— 宿主那条「不引框架」是给本仓自己的面板定的（体积 / 缓存 / 一致性），
> 插件是独立打包的产物，不受它约束。

## 5. 依赖：`dependencies`

插件要用的**外部文件**（引擎二进制、模型、字体…）写在清单里，宿主会在**装插件时一并拉好**，
失败就整体回滚。两种形态：

```json
"dependencies": [
  {
    "type": "file",
    "url": "https://example.com/librespot.exe",
    "dest": "bin/librespot.exe",
    "sha256": "9f2c…"
  },
  {
    "type": "npm",
    "package": "marked",
    "version": "^12.0.0"
  }
]
```

| 形态 | 字段 | 说明 |
|---|---|---|
| `file`（缺省） | `url` | **必须 https**。明文 http 会被直接拒绝 |
| | `dest` | 相对插件目录的落点，如 `bin\librespot.exe`。**不许越界**（不能含 `..`、不能绝对路径） |
| | `sha256` | 可选但要**强烈建议**写：写了就逐字节校验，不匹配即拒绝安装 |
| `npm` | `package` / `version` | 走 `npm install --prefix <插件目录>`，装进插件目录的 `node_modules\`。**要求用户机器上有 Node.js**，没有会如实报错 |

单个依赖文件上限 512 MB、最多 32 条。装完之后插件用**相对路径**引用它们：

```js
// 插件自己怎么找到已装好的依赖：入口 URL 反推出插件目录
const dir = new URL(".", import.meta.url);            // .../Modules/pomodoro/
const engine = new URL("bin/librespot.exe", dir).pathname;
```

> **拿不准就别加依赖**：能纯 JS 实现的功能，写成单文件最省事也最不容易坏。
>
> `file` 依赖的地址必须是 **https + 免鉴权直链**。上游不发预编译包的东西（如 librespot），
> 要么自己构建后挂到**公开仓库的 Release 资产**上（本仓官方插件就是这么做的：
> `LythrumMoon/lunac-plugins` 的 `librespot-0.8.0`），要么改用纯 JS 方案。

## 6. 怎么把它装进 Lunac

**手工安装（开发 / 自用）**：把插件文件夹直接放进 `<exe 根>\Modules\<id>\`，
然后打开「设置 → 插件 → 重新扫描」（或重启 Lunac）即可用。

**打包分发**：把**插件目录本身**压成 zip（`lunac-plugin.json` 在 zip 根，
或多一层顶层目录，两种形状都认），传到任何 **https 可直链、无需鉴权**的地方
（私有仓库的 raw / Release 链接**不行** —— 终端用户下不到），
再把一条记录加进**市场索引**（Lunac 官方的是公开仓库
[`LythrumMoon/lunac-plugins`](https://github.com/LythrumMoon/lunac-plugins) 的 `index.json`；
宿主常量 `plugin_market::INDEX_URL` 指向它）：

```json
{ "id": "pomodoro", "name": "番茄钟", "description": "25 分钟专注计时器",
  "version": "1.0.0", "url": "https://example.com/pomodoro-1.0.0.zip",
  "keywords": ["pomodoro", "番茄"], "icon": "🍅", "homepage": "https://example.com" }
```

> 官方插件的发布链**有序，且顺序不许跳**（改插件源码后一定走完）：
> `scripts\build-plugins.ps1`（构建 + 生成清单 + 打 zip，**并且先把 dev 实例的
> `target\debug\Modules\<id>\` 更新掉**）→ 在 dev 里验证 →（要随安装包发新版才跑
> `scripts\build-release.ps1`）→ `scripts\publish-plugins.ps1`（拷进插件仓库并更新 `index.json`）。
> **先 publish 后 build 会把上一版推上去** —— 用户被喊更新却拿不到新东西。

**如果入口不是手写的单文件 ESM**（比如用了 TypeScript 或 npm 包），
发布前要自己先打包成一份 ESM。用 Vite 的话：

```js
// vite.config.js（打包插件用，和聊天前端无关）
export default {
  build: {
    lib: { entry: "index.js", formats: ["es"], fileName: "index" },
    rollupOptions: { output: { inlineDynamicImports: true } },  // 依赖全打进同一个文件
  },
};
```

> ⚠️ **产物必须是一份单文件**。用 Vite 一次打**多个插件入口**时，Rollup 会把共享模块
> 提到 `dist/<chunk 名>/chunk-*.js` 这类**公共 chunk** 里，而插件包只搬走自己那个目录
> ⇒ chunk 丢失、`import` 404、插件打开直接失败。
> 正确做法：**一个入口一次构建**（本仓官方插件的做法见
> `app/vite.plugins.config.ts` + `scripts/build-plugins.ps1`）。
> 另外注意：Vite 的 CLI **不支持**一个配置文件导出多份配置（那是 Rollup 的用法），
> 别想着「一次 build 打完所有插件」。

> **CSP 约束**：插件代码只能从**本地磁盘**加载（经 asset 协议），
> **不能 `import()` 任何 http/https/CDN 地址** —— 那是宿主的安全边界，不是配置项。
> 需要第三方库就把它的代码打进你的入口文件里。

## 7. 规则与安全（写之前先读）

- **插件是可执行代码**。`type: "html"` 的返回值会被 `innerHTML` 插入结果区 ——
  **不要拼接用户输入到 HTML 里**（会变成注入）。要显示用户文本就自行转义：
  `s.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;")`。
- **id 就是目录名**，只允许 `[a-z0-9._-]`、不以 `.` 开头；宿主用 `.staging-*` / `.old-*`
  这类点号目录做中间态，别去碰它们。
- **入口必须在插件目录内**，`entry` 不能是绝对路径、不能含 `..`。
- **装/卸是原子的**：宿主先解压到临时目录、校验通过才改名进正式位置；升级时旧版本先备份，
  新包或依赖失败会回滚 —— 所以你可以放心「同 id 再装一次」当升级用。
- **同一 id 只能有一个目录**。想改内容就改这个目录，不要在别处再放一份。
- 一个插件进程里只有**一个模块实例**（宿主按 id 缓存）。升级会清缓存，但**正在打开的面板
  不会被强制换掉** —— 重新点开即可。

## 8. 让 Lunac 自己写一个插件（给 AI 的操作步骤）

1. 读本文件（绝对路径在系统提示词的 `Environment:` 块里，通常就是
   `<exe 根>\Modules\README.md`）。
2. 与用户确认三件事：**插件 id**（小写英文）、**它做什么**、**搜索关键词**。
3. 在 `<exe 根>\Modules\<id>\` 下创建两个文件：`lunac-plugin.json` 与 `index.js`。
   入口用**纯 ESM**，不要 TypeScript、不要构建步骤、不要外部 CDN。
   清单里凡是**要动窗口**的需求都写进 `permissions`（见第 2 节；不写就不生效）；
   要**照搬某个内置插件的面板与交互**就写 `reuse`（见第 2 节「复用内置插件监听」）。
4. 需要交互就补 `attach(root)` / `detach()`；文案**直接写死**（宿主的 `t()` 不认识你起的 key）。
5. **不用叫用户去点「重新扫描」** —— 宿主嗅探到本回合写入了 `Modules\<id>\`（Write / Edit / PowerShell 等），
   会在**这个回合结束时自动重扫**并注册，搜索框里立刻就能搜到。
   （只有手工拷文件进去、绕开了 Lunac 的情况，才需要去 设置 → 插件 → 重新扫描，或重启。）

**不要做的事**（做了会破坏「卸载 = 完全不存在」这条硬约束）：

- ❌ 不要去改主程序源码（`app/src/...`）来「注册」你的插件 —— 宿主靠扫本目录发现插件，
  为某个插件改宿主代码，会让它在卸载后仍留在主程序里。
- ❌ 不要把插件拆到多个目录、也不要依赖别处再放一份（同 id 只能有一个目录）。
- ❌ 不要在插件里 `import` 网络地址（CSP 会拦掉），也别指望宿主替你装 npm 依赖以外的环境。

# Lunac 插件（Modules）开发规范

> 这份文档既是**给人看的**说明书，也是**给 Lunac 自己的 AI 看的**规范 ——
> 它被放在 `<exe 根>\Modules\README.md`，agent 的系统提示词里给了这个绝对路径。
> 想让 Lunac 帮你写一个插件，直接说「照 Modules\README.md 的规范给我做一个 XXX 插件」即可，
> **不需要 dev 版、也不需要任何前端构建环境**：插件就是几个纯文本文件。

---

## 1. 插件是什么

Lunac 的搜索结果里，除了内置功能（设置 / 备忘录 / 转换…），还能出现**插件**。
插件 = 一个文件夹 + 一份清单 + 一个 ESM 入口。装上之后它在同一个搜索框里被搜到、点开即在结果区渲染。

**一切都以磁盘为准**：`<exe 根>\Modules\<插件 id>\` 这个目录存在、清单与入口都合法，
插件就存在。没有注册表、没有中心数据库 —— 目录就是唯一真相源（卸载 = 删目录）。

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

> 官方插件的发布链是两条命令：`scripts\build-plugins.ps1`（构建 + 生成清单 + 打 zip）→
> `scripts\publish-plugins.ps1`（拷进插件仓库并更新 `index.json`）。

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
4. 需要交互就补 `attach(root)` / `detach()`；需要宿主翻译就走 `globalThis.__lunac_host`。
5. 告诉用户「打开 设置 → 插件 → 重新扫描」即可使用（无需重启）。

# 面板骨架模板 / DOM 契约（`reuse` 专用）

> 这份文件是 [README.md](./README.md) 第 2 节「复用内置插件监听：`reuse`」的**可复制补充**。
> 什么时候读它：你想「照抄某个内置面板的样式与交互」，在 `lunac-plugin.json` 里写
> `reuse: ["<基础插件 id>"]` 的时候。
>
> 一份纯文本资产，**不需要任何宿主改动** —— 里面的 HTML 骨架逐行抄进你自己的 `index.js` 即可。

---

## 1. 为什么 `reuse` 要「照抄 id / class」

磁盘插件的结果 HTML 由**你自己的** `execute(input)` 产出；而那些按钮 / 输入框靠的是
**编译进主 bundle 的一套监听**（`app/src/plugins/attach.ts` 的 `attachBaseListeners`）。
`reuse` 做的事是：在调用**你自己的** `attach(root)` 之外，**再把被复用插件的那套监听挂到同一个
`root` 上**（`app/src/plugins/attach.ts:59-75`）。

那套监听是用 `root.querySelector("#id")` / `root.querySelector(".class")` 找元素的
（少数例外见 §5 的 `tool-editor` —— 它用**全局** `document`）。
=> **你产出的 HTML 里节点 id / class 必须与内置面板逐一对齐，监听才找得到元素**；
抄错一个 id（或换个 class），轻则该控件不挂监听（点了没反应），重则整块 `attach` 提前 `return`
（`translate` 就是「五个必需节点缺一个就全不挂」，见下）。

---

## 2. 通用骨架：自己画面板（**不用** `reuse`）

不需要复用内置交互时，最省事的就是自绘 + 自己挂监听；**不要**写 `reuse`：

```js
// index.js —— 自绘面板的最小骨架
export default {
  async execute(input) {
    return {
      type: "html",
      content:
        '<div class="demo-root">' +
        '<button type="button" id="demo-go">点我</button>' +
        '<span id="demo-out"></span>' +
        "</div>",
    };
  },
  // 结果 HTML 插进 DOM 之后，宿主调它；root = 结果区容器
  attach(root) {
    const btn = root.querySelector("#demo-go");
    const out = root.querySelector("#demo-out");
    let n = 0;
    btn?.addEventListener("click", () => {
      if (!root.isConnected) return; // 面板已被换掉 => 自己停（不依赖宿主一定调到 detach）
      n += 1;
      if (out) out.textContent = String(n);
    });
  },
  detach() {
    // 自己开的定时器 / 监听 / 订阅在这里收尾（这里只演示，无长驻资源）
  },
};
```

要点（与 README 第 3 节同源）：`attach` 里一律 `root.querySelector(...)`，**不要用全局
`document`**；`attach` / `detach` 里自己开的东西自己收。

---

## 3. 可复用的基础插件一览

表里的「监听入口」是宿主实际调用的函数（`app/src/plugins/attach.ts:27-57` 的 `switch`）。
**只有这些 id 能被 `reuse`**；写别的（拓展插件 / 不存在的 id）只会记一条 `console.warn`、不生效。

| `reuse` id | 对应面板 | 监听入口 | root 作用域 | 建议 |
|---|---|---|---|---|
| `translate` | 翻译 | `attachTranslateListeners(root)` | **root** | 推荐：契约最小 |
| `quick-launch` | 快速启动 | `attachQuickLaunchListeners(root)` | **root** | 一般 |
| `memo` | 备忘录 | `attachMemoListeners(root)` | **root** | 一般 |
| `settings` | 设置 | `attachSettingsListeners(root)` | **root** | 不建议：契约极大 |
| `tool-editor` | 工具编辑器 | `attachToolEditorListeners()` | **全局 document** | 不建议：见 §5 |

> `reuse` 最多 4 条（`MAX_REUSE`），条目必须合法（`[a-z0-9._-]`）。

---

## 4. 逐面板 DOM 契约（要抄哪些 id / class）

### 4.1 `translate`（推荐，契约最小）

必需节点 —— **五个缺一个就整块不挂**（`app/src/plugins/builtin/translate.ts:120-126`）：

| 必需 id | 形态 | 出处 |
|---|---|---|
| `#xl-input` | `<textarea class="xl-input">` | `translate.ts:106` |
| `#xl-from` | `<select class="xl-select">` | `translate.ts:109` |
| `#xl-to` | `<select class="xl-select">` | `translate.ts:111` |
| `#xl-go` | `<button class="xl-go">` | `translate.ts:112` |
| `#xl-result` | `<div class="xl-result">` | `translate.ts:114` |

另：结果区里按钮 **`#xl-ai`** 会在每次重绘结果后重新绑（`translate.ts:133-134`）——
你自己 `execute` 的初始 HTML **不用**带它（它由被复用的监听在渲染结果时插入）。

清单 + 入口（可直接复制）：

```json
{
  "id": "my-translate",
  "name": "我的翻译面板",
  "description": "复用内置翻译面板的骨架",
  "keywords": ["translate", "翻译", "我的翻译"],
  "version": "1.0.0",
  "entry": "index.js",
  "reuse": ["translate"]
}
```

```js
// index.js —— 复用内置「翻译」面板的 HTML 骨架（reuse: ["translate"]）
export default {
  async execute(input) {
    return { type: "html", content: panelHtml((input || "").trim()) };
  },
};

// 目标语言（照抄内置那一套取值即可；这里只列几种作示例）
const LANGS = ["zh-CN", "zh-TW", "ja", "ko", "en"];

function esc(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function options(cur) {
  return LANGS.map(function (c) {
    return '<option value="' + c + '"' + (c === cur ? " selected" : "") + ">" + c + "</option>";
  }).join("");
}

function panelHtml(seed) {
  return (
    '<div class="xl-root">' +
    '<textarea id="xl-input" class="xl-input" rows="3" spellcheck="false">' + esc(seed) + "</textarea>" +
    '<div class="xl-row">' +
    '<select id="xl-from" class="xl-select"><option value="auto">auto</option>' + options("auto") + "</select>" +
    '<span class="xl-arrow">&#8594;</span>' +
    '<select id="xl-to" class="xl-select">' + options("zh-CN") + "</select>" +
    '<button type="button" id="xl-go" class="xl-go">翻译</button>' +
    "</div>" +
    '<div id="xl-result" class="xl-result"></div>' +
    "</div>"
  );
}
```

### 4.2 `quick-launch`

| 要抄的节点 | 作用 | 出处 |
|---|---|---|
| `.ql-add-btn` | 「添加启动项」按钮 | `quick-launch.ts:88` / 监听 `:130` |
| 每行 `[data-custom-path="<路径>"]` | 一条启动项（点击即启动） | `quick-launch.ts:106` / 监听 `:154` |
| `.ql-remove`（行内） | 删除该条 | `quick-launch.ts:112` / 监听 `:160` |

> 面板行还会带 `.result-item` / `.result-item-badge.app-badge` 等样式类（可选，只影响观感）；
> **能点的那三处必须逐字对上**。

### 4.3 `memo`

根容器 **`.memo-root`**（`memo.ts:183`），并带 `data-view="main"` 或 `data-view="history"`
（`memo.ts:108` / `:171`）。两种视图的节点都要抄（`render()` 会用 `root.innerHTML = html`
整体重画并重挂，`:186-190`）。

主编辑视图（`memo.ts:106-127`）：

| id | 形态 |
|---|---|
| `#memo-textarea` | 正文 `<textarea>` |
| `#memo-images` | 图片条容器 |
| `#memo-save-btn` | 保存 |
| `#memo-autosave-hint` | 小字提示（`memo` 会写它的 textContent） |
| `#memo-clear-btn` | 清空全部（两段式确认） |
| `#memo-history-btn` | 打开历史 |
| `#memo-tag-dialog` / `#memo-tag-input` / `#memo-tag-ok` / `#memo-tag-skip` | 保存后的「标识」对话框 |

历史视图（`memo.ts:153-178`）：

| id / class | 形态 |
|---|---|
| `#memo-back-btn` | 返回主界面 |
| `#memo-hist-list` | 列表容器 |
| 每条 `.memo-hist-item[data-id="<id>"]` | 行 |
| `.memo-copy` / `.memo-edit` / `.memo-del` | 行内 复制 / 编辑 / 删除（按钮靠 `data-id` 认条目，`:165-167`） |

### 4.4 `settings`（结构契约；整块抄很重，谨慎）

根 `.settings-layout`（`settings.ts:3769`）＝ 左侧 `.settings-sidebar`（`:3770`）+ 右侧
`.settings-content`（`:3787`）。

- 侧栏**恰好五项**：`.settings-sidebar-item[data-cat="general|appearance|ai|search|plugins"]`
  （`settings.ts:3771-3785`；这也是 ui-style.md §界面 9 的硬约束）。
- 每个分类一个面板：`.settings-pane[data-pane="<cat>"]`（例：`data-pane="general" id="sp-general"`
  `settings.ts:168`）。侧栏点击靠 `data-cat` ↔ `data-pane` 对应切换（`settings.ts:2659-2679`）。
- 面板里控件 id 极多（如 `#settings-hotkey-btn` `#settings-autostart` `#settings-provider`
  `#settings-model` `#settings-baseurl` `#settings-apikey` `#settings-save-ai-btn` …，
  `settings.ts:2669-3560`）。**想真的复用，就得把对应 pane 的 HTML 整段抄过来**；
  只抄一小部分会得到「一半控件是死的」。
  => 结论：除非你要做设置面板的**完整变体**，否则**别用 `reuse: ["settings"]`**，自绘更省。

### 4.5 `tool-editor`（注意：它用全局 `document`，不是 root）

`attachToolEditorListeners()` **不收 `root` 参数**，它直接：
`document.getElementById("results-list")`（`tool-editor.ts:49-51`）与
`document.getElementById("tool-new-btn")`（`:211`），并在行内找 `.tool-btn-edit` / `.tool-btn-del`
（`:219` / `:228`，都靠 `data-filename`，`:222` / `:231`）。

=> 只有当你的插件**渲染进当前窗口的 `#results-list`**（内嵌形态）且带 `#tool-new-btn` /
`[data-filename]` / `.tool-btn-edit` / `.tool-btn-del` 时才生效。
**用了 `layout.takeover`（root = 文档根）时它找不到 `#results-list`，永远不生效。**
=> 结论：一般**不要** `reuse: ["tool-editor"]`。

---

## 5. 常见错误（会发生什么）

1. **id / class 抄错一个** => 监听找不到元素。`translate` 是「五个必需节点缺一个就整块不挂」
   （点了完全没反应）；`memo` / `quick-launch` 是「某个具体按钮是死的」。症状统一是
   「**只有后端/界面有反应，按钮点了没反应**」。
2. **写 `reuse` 却不照抄对应 HTML** => 等于没写（监听挂上了，但没有元素可绑）。
3. **`tool-editor` 用 `layout.takeover`** => 它的全局 `document` 查 `#results-list` 落空，
   永远不生效（见 §4.5）。
4. **`reuse` 写了拓展插件 id**（`music` / `ocr` / `clipboard-history` …）或写了不存在的 id
   => 宿主只记一条 `console.warn`，**不生效**（`attach.ts:72`）。`reuse` **只认基础插件**。
5. **期望 `reuse` 改掉搜索结果里的名字 / 图标** => 不会。搜索结果显示的仍是你清单里的
   `name` / `icon` / `keywords`；`reuse` 只多挂一套**结果区里的**监听（README 第 2 节第 3 条）。
6. **自己的 `attach(root)` 里 `root.innerHTML = html` 重画** => 把复用监听刚绑上的节点换掉，
   按钮当场变死。`memo` 之所以敢重画，是因为它**重画后立刻重挂**（`memo.ts:186-190`）——
   你若要模仿，必须自己同样重挂；否则别重画。
7. **`reuse` 超过 4 条 / id 含非法字符** => 清单校验不过，`reuse` 整段被拒。
8. **以为 `reuse` 能拿到被复用插件的内部状态** => 拿不到。监听只操作它自己那套 DOM + 走宿主
   命令；你没法读它的 JS 变量。

---

## 6. 依据（本文件所有断言的出处）

| 断言 | 出处 |
|---|---|
| `reuse` 的挂载机制（在插件自己的 `attach` 之外追加基础监听） | `app/src/plugins/attach.ts:59-75` |
| 可复用 id 的 `switch` 全表 | `app/src/plugins/attach.ts:27-57` |
| `translate` 五个必需节点 + 早退 | `app/src/plugins/builtin/translate.ts:105-126` |
| `quick-launch` 三处可点节点 | `app/src/plugins/builtin/quick-launch.ts:85-118`、`:128-183` |
| `memo` 主/历史两视图节点 | `app/src/plugins/builtin/memo.ts:106-178`、`:182-245` |
| `settings` 结构 + 五项侧栏 | `app/src/plugins/builtin/settings.ts:3768-3794`、`:2659-2679` |
| `tool-editor` 用全局 `document` | `app/src/plugins/builtin/tool-editor.ts:49-51`、`:164-258` |
| 「卸载 = 完全不存在」：不要为磁盘插件改主程序 | `docs/forbidden.md` §禁用 51 |

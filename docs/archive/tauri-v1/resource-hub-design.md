# 资源整合平台（Resource Hub）设计说明（2026-10-18）

> **本文档是什么**：一份**设计记录（非规范）**，不是已实现功能。
> 目标：设计一个可同时以 **Lunac 拓展插件** 与 **独立跨端项目（Windows / iOS / Android）** 交付的
> 漫画 / 小说 / 番剧 / 电视剧资源整合平台，并对 iOS App Store 的合法发布路径做调研与结论。
>
> 参考项目：[Venera](https://github.com/venera-app/venera)、[venera_comic_source](https://github.com/handahao666-boop/venera_comic_source)、
> [uzVideo](https://github.com/YYDS678/uzVideo)、[uzVideo-extensions](https://github.com/YYDS678/uzVideo-extensions)、
> [Kazumi](https://github.com/Predidit/Kazumi) / [KazumiRules](https://github.com/Predidit/KazumiRules)。
>
> 现行规范仍以 `docs/` 根下 6 份文档为准（见 `README.md`）；本文只作方案取证与拍板底稿。

> **本轮拍板（2026-10-18）**
> - **Windows 只做 Lunac 拓展插件**，不再做独立 Windows 桌面版。
> - **iOS**：**暂不考虑 App Store**；完整版只走自签名 / LiveContainer + SideStore。
> - **预算**：**零成本优先**（免费 Apple ID + 7 天刷新）；不购买 Apple Developer Program。
> - **Android**：**暂不走 Google Play**，只发 APK。
> - **许可**：**MIT**；声明为**个人项目、非商用、无任何广告**。
> - **不尝试用远程仓库 URL 绕过 App Store 审核**（详见 9.2.3）。
> - **Windows 自签名路线**：Impactor / SideStore / LiveContainer（详见 9.2.8）。
> - **国内网络**：自建 Anisette 服务器 + 自定义服务器列表（详见 9.2.9）。
> - **开发方式**：AI 侧与插件侧**并行开发**，先冻结接口（详见第十二章）。

---

## 一、结论先行

1. **不要自造来源格式**。参考项目的护城河都是来源生态（Venera 的 JS 源、uzVideo 的扩展包、Kazumi 的 XPath 规则），
   所以本项目的核心是 **「统一资源源协议（RSP）+ 三个适配器」**，而不是再发明一种规则语言。
2. **一套内核、两种外壳（本轮定）**：
   - **Windows = Lunac 拓展插件**：`apps/lunac-plugin`（单文件 ESM + `lunac-plugin.json`）；
   - **iOS / Android = 独立应用**：`apps/standalone`（Tauri 2），复用同一套 TS 内核与 UI。
   - **不再规划独立的 Windows 桌面版** —— Windows 的交付形态就是插件。
3. **分发形态（本轮定）**：
   - Windows：Lunac 插件市场 / 本地 `Modules` 目录。
   - iOS：**只走自签名 / LiveContainer + SideStore**；**App Store 暂不考虑**（Lite 方案仅留档）。
   - Android：**暂不走 Google Play**，只发官网 / GitHub Release 的 APK。
   - ⇒ 所有平台都用 `full` 能力；`store` 构建暂不产出。
4. **App Store 结论（详见 9.2）**：完整聚合模式**基本不可能通过审核**；即使 Lite 也需要 99 USD/年会员，
   本轮预算不支持 ⇒ **暂不投入**，保留调研结论供以后使用。
5. **不能用「远程仓库资源 URL」绕过审核**：2.5.2 本来就覆盖**下载的解释型代码**；绕审属于 Apple 明文的
   「trick the review process」，可导致下架并**开除开发者账号**（详见 9.2.3）。
6. **账号**：本轮**不购买** Apple Developer Program；用**免费 Apple ID + 7 天证书**自签名。
   若将来要上 App Store / TestFlight，再评估 99 USD/年会员（详见 9.2.4）。
7. **许可与声明（本轮定）**：项目采用 **MIT**；声明为**个人项目、非商用、无任何广告**；
   **不内置任何第三方来源脚本 / 规则**，来源由用户自行导入（详见 9.6）。
8. **Lunac 插件形态有一个硬前提**：当前宿主 CSP / 桥不提供外网 JSON 与媒体代理，插件只能加载 `https` 图片。
   要落地，必须先加 **宿主 API v3（`net.request` + `media.proxy` + 插件 KV）**；否则插件只能做声明式来源的
   「轻量阅读器」，完整能力只能放在独立应用里。

---

## 二、参考项目拆解

| 项目 | 技术栈 | 来源模型 | 分发现实 | 可复用结论 |
|---|---|---|---|---|
| **Venera** | Flutter + Rust + `flutter_qjs` | 每个来源 = 一个 JS `class extends ComicSource`；`index.json` 列 `{name, fileName/url, key, version}` | GPL-3.0；仓库 2026-04 归档；iOS 走 AltStore（仓库带 `alt_store.json`） | JS 源 API 面最大（Network / Html / Crypto / UI / Types），适合做**完整模式**的兼容目标 |
| **venera_comic_source** | JS 源集合 | 40+ 个漫画源，含 MangaDex / 拷贝漫画 / 漫画柜等 | 第三方镜像仓库，随 Venera 归档仍在更新 | 证明「一份 index.json + 一堆 .js」的分发形态可用 |
| **uzVideo** | Flutter | 单 JS 运行时；扩展实现 `WebApiBase`，按 type 分 101 视频源 / 200 首页 / 300 网盘 / 400 弹幕 | 无 LICENSE 文件（默认保留所有权利）；iOS / Android / Win / Mac | 视频源与弹幕的接口抽象值得借鉴；**不要复制其代码** |
| **uzVideo-extensions** | 纯 JS | `uzAio.zip` 订阅包；`//@name` 注释生成元数据；可加密并绑定 `codeID` | 订阅链接分发 | 订阅包 + 元数据注释 + 代码加密的供应链模型 |
| **Kazumi** | Flutter + media-kit + mpv | **最多 5 行 XPath** 的自定义规则；`KazumiRules` 托管 `index.json` | GPL-3.0；iOS 明确要求**自签名**；Windows 有 SignPath 免费签名 | 声明式规则是**最低成本、最安全**的来源形态，应作为 v1 第一条落地路径 |

### 2.1 共性判断

- 三者都是「**壳 + 来源生态**」：应用本身不持有内容，来源由社区维护。
- 来源格式碎片化严重：JS 类、JS 函数、JSON + XPath、甚至 `webview` 嗅探。
- 合规风险不在播放器，而在「**应用是否分发、推荐、索引了未授权来源**」。
- 维护成本主要来自来源失效，而不是播放器本身 ⇒ 必须把「来源更新」做成独立于应用发版的通道。

---

## 三、产品定义

### 3.1 定位

一个**本地优先、来源可插拔**的内容聚合器：

- 漫画：图片章节阅读（分页 / 纵向 / 条漫），预加载与离线缓存。
- 小说：章节文本阅读（TXT / HTML / EPUB），排版、进度、书签。
- 番剧 / 电视剧：HLS / MP4 播放，字幕、弹幕（可选）、外部播放器兜底。

**不持有内容、不提供内容**：只做「来源协议的解析器 + 阅读 / 播放器 + 书架」。

### 3.2 用户旅程

```
搜索 / 发现 → 聚合结果（跨来源去重） → 作品详情（来源切换）
→ 章节列表 → 阅读器 / 播放器 → 进度入库 → 书架 / 历史 / 下载 → 同步
```

### 3.3 功能矩阵

| 优先级 | 能力 |
|---|---|
| **P0** | 多来源搜索、作品详情、章节列表、漫画阅读、小说阅读、视频播放、书架 / 历史 / 进度、来源导入与管理 |
| **P1** | 聚合去重、来源切换、下载与离线、WebDAV 同步、字幕、外部播放器、NSFW 开关与年龄门 |
| **P2** | 弹幕、时间表、更新提醒、TTS、投屏 / DLNA、跨设备续播、来源市场与签名校验 |
| **非目标** | 自建内容库、账号体系、付费 / 内购、社交评论、爬虫托管服务 |

---

## 四、总体架构

### 4.1 分层

```
┌─────────────────────────────────────────────────────────────┐
│ 外壳层                                                       │
│  · Lunac 插件：单文件 ESM（execute / attach）+ 独立悬浮窗      │
│  · 独立应用：Tauri 2（iOS / Android）                          │
├─────────────────────────────────────────────────────────────┤
│ UI 层（rh-ui / rh-reader / rh-player，无框架 Web Components） │
│  发现 · 搜索 · 详情 · 书架 · 阅读器 · 播放器 · 来源管理         │
├─────────────────────────────────────────────────────────────┤
│ 领域层（rh-core）                                             │
│  聚合搜索 · 去重排序 · 书架 / 进度 · 下载队列 · 同步 · 缓存策略  │
├─────────────────────────────────────────────────────────────┤
│ 来源层（rh-source-sdk + rh-adapters）                         │
│  RSP 协议 · 声明式 DSL · JS 源运行时 · Kazumi/Venera/uz 适配器  │
├─────────────────────────────────────────────────────────────┤
│ 宿主适配层（Host Adapter）                                    │
│  Lunac：__lunac_host v3（network / media / storage）           │
│  独立：Rust（reqwest / rquickjs / sqlite / 本地媒体代理）       │
└─────────────────────────────────────────────────────────────┘
```

**关键原则**：领域层与 UI 层**不感知**来源格式与宿主差异；所有差异收敛在「来源层」与「宿主适配层」。

### 4.2 仓库结构（双形态同仓）

```
resource-hub/
  packages/
    rh-core/          # 纯 TS：模型、聚合搜索、进度、下载状态机
    rh-source-sdk/    # RSP 类型、清单校验、DSL 解释器、JS 运行时接口
    rh-adapters/      # kazumi / venera / uzvideo / opds / local
    rh-ui/            # Web Components + 设计 token
    rh-reader/        # 漫画 / 小说渲染器
    rh-player/        # hls.js + 原生 HLS 分支 + 字幕
  apps/
    lunac-plugin/     # Vite lib 模式 → dist/index.js（单文件）+ lunac-plugin.json
    standalone/       # Tauri 2：src（复用 packages）+ src-tauri
  sources/            # 仅放自研 / 已授权 / 示例来源
  docs/               # 协议、适配器矩阵、发布说明
```

- 构建顺序：`rh-*` → `apps/lunac-plugin` → `apps/standalone`。
- 插件入口必须**单文件 ESM**（宿主硬约束，见 `Modules/README.md` 第 6 节），禁止公共 chunk。
- 独立应用复用同一份 `packages`，Tauri `src` 只做宿主适配与路由。

### 4.3 两种外壳差异（Windows 插件 vs iOS / Android 独立应用）

| 维度 | Lunac 插件（Windows） | 独立应用（iOS / Android） |
|---|---|---|
| 运行位置 | Lunac 主窗 / 插件悬浮窗 | 独立进程 / 独立窗口 |
| 网络 | 依赖宿主 API v3（当前缺失） | Rust `reqwest` 直连 |
| JS 来源 | 需要宿主 QuickJS 桥（或仅声明式） | 内置 QuickJS（full 模式） |
| 存储 | 宿主插件 KV（新增）/ IndexedDB | SQLite |
| 媒体 | `https` 图片直连；视频需宿主代理 | 本地代理 + 原生播放器 |
| 分发 | Lunac 插件市场 / `Modules` 目录 | iOS：侧载 IPA + App Store Lite；Android：APK（不上 Play） |
| 合规 | Windows 桌面端，无商店审核（仍守版权法） | iOS 分 `full` / `store`；Android 只发 `full` |

---

## 五、统一资源源协议 RSP v1

### 5.1 设计取舍

- **协议只描述能力，不规定实现**：同一个来源可以用声明式规则、JS 模块或内置适配器实现。
- **来源必须可版本化、可校验、可撤销**：清单 + 签名 + 最小宿主版本。
- **不做「万能 DSL」**：声明式只覆盖 HTTP + 提取 + 分页；复杂站点交给 JS 源。
- **不内置第三方来源**：仓库只放示例与自研来源，避免许可与合规风险。

### 5.2 核心数据模型

```ts
type Kind = "comic" | "novel" | "anime" | "tv";

interface WorkRef { sourceId: string; workId: string; kind: Kind }

interface Work {
  ref: WorkRef;
  title: string;
  altTitles?: string[];
  author?: string;
  cover?: string;
  description?: string;
  tags?: string[];
  status?: "ongoing" | "completed" | "unknown";
  updatedAt?: number;
  extra?: Record<string, unknown>;
}

interface Chapter {
  id: string;
  workId: string;
  title: string;
  index: number;
  group?: string;         // 卷 / 季
  publishedAt?: number;
}

interface ComicPage { index: number; url: string; headers?: Record<string, string> }
interface TextChapter { chapterId: string; format: "text" | "html" | "epub"; content: string }
interface Playback {
  url: string;
  type: "hls" | "mp4" | "dash" | "webview";
  headers?: Record<string, string>;
  subtitles?: { label: string; url: string; lang?: string }[];
  duration?: number;
}
```

### 5.3 来源清单（`rsp.json`）

```json
{
  "rsp": 1,
  "id": "example.comic",
  "name": "示例漫画",
  "version": "1.0.0",
  "kind": ["comic"],
  "engine": "declarative",
  "entry": "rule.json",
  "baseUrl": "https://example.com",
  "homepage": "https://example.com",
  "license": "user-imported",
  "minHost": "1.0.0",
  "rateLimit": { "rps": 2, "burst": 4 },
  "capabilities": ["search", "detail", "chapters", "pages"],
  "nsfw": false,
  "headers": { "User-Agent": "Mozilla/5.0 …" }
}
```

| 字段 | 说明 |
|---|---|
| `engine` | `declarative`（JSON 规则）/ `js`（JS 模块）/ `builtin`（适配器内置） |
| `capabilities` | 必须与实现一致；缺能力时 UI 隐藏对应入口 |
| `minHost` | 低于此版本的宿主拒绝加载并说明原因 |
| `rateLimit` | 宿主强制限速（令牌桶），保护来源站点 |
| `nsfw` | 商店模式下一律不加载；完整模式需年龄门 |

### 5.4 来源接口

```ts
interface ResourceSource {
  manifest: SourceManifest;
  search?(q: SearchQuery): Promise<Paged<Work>>;
  explore?(section?: string, page?: number): Promise<Paged<Work>>;
  detail?(ref: WorkRef): Promise<Work>;
  chapters?(ref: WorkRef): Promise<Chapter[]>;
  pages?(chapter: Chapter): Promise<ComicPage[]>;        // comic
  content?(chapter: Chapter): Promise<TextChapter>;      // novel
  play?(chapter: Chapter): Promise<Playback[]>;          // video
  auth?: AuthProvider;
}
```

- 所有方法必须支持 `AbortSignal`（超时 / 切换页面 / 取消）。
- 失败必须抛结构化错误：`{ code: "auth" | "network" | "parse" | "blocked" | "unknown", message }`。
- 单来源超时默认 8s；搜索聚合使用 `Promise.allSettled`，**失败来源要显示原因**，不得静默消失。

### 5.5 声明式规则（v1 优先）

覆盖「HTTP 请求 + 列表提取 + 分页」三类站点：

```json
{
  "search": {
    "request": { "method": "GET", "url": "/search?q=@keyword&page=@page" },
    "list": { "css": ".book-item" },
    "fields": {
      "id":   { "css": "a", "attr": "href", "regex": "/book/(\\d+)" },
      "title":{ "css": ".title", "text": true },
      "cover":{ "css": "img", "attr": "data-src" }
    },
    "paging": { "type": "page", "param": "page", "start": 1 }
  },
  "chapters": {
    "list": { "xpath": "//ul[@id='chapter-list']/li/a" },
    "fields": { "id": { "attr": "href" }, "title": { "text": true } }
  },
  "pages": {
    "list": { "xpath": "//div[@id='images']/img" },
    "fields": { "url": { "attr": "src" } }
  }
}
```

- 提取器：`css`（默认）、`xpath`（Kazumi 兼容）、`jsonpath`、`regex`。
- 所有相对 URL 以 `baseUrl` 解析；重定向上限 5 次。
- 正则必须设置长度上限与执行超时（防 ReDoS）。

**字段约定**（`fields` 的 key 由执行器直接映射为 RSP 类型）：

| key | 消费位置 | 说明 |
|---|---|---|
| `id` | `WorkRef.workId` / `Chapter.id` | 作品或章节的稳定 id；缺省时回退 `url` |
| `title` | `Work.title` / `Chapter.title` | 必填（搜索行） |
| `subtitle` / `author` / `description` / `tags` | `Work` | `tags` 用逗号分隔 |
| `cover` | `Work.cover` | 相对地址按 `baseUrl` 解析 |
| `url` / `src` | `ComicPage.url` | 图片页优先 `url`，其次 `src` |
| `content` / `text` | `TextChapter.content` | 正文节点 |
| `playUrl` | `Playback.url` | 播放地址；缺省回退 `url` |
| `type` | `Playback.type` | `hls/mp4/dash/webview`；缺省按 `.m3u8` 推断 |

**请求占位符**：`@keyword`、`@page`、`@workUrl`（作品详情页）、`@chapterUrl`（章节页）、`@chapterId`、`@value`（字段模板内）。`@workUrl` 由 `WorkRef.workId` 解析；Kazumi 的章节页即详情页。

### 5.6 三个适配器

| 适配器 | 输入 | 映射方式 | v1 覆盖 | 明确不支持 |
|---|---|---|---|---|
| **Kazumi** | `KazumiRules/*.json`（api 1 / 9） | `searchURL/searchList/searchName/searchResult` → `search`；`chapterRoads/chapterResult` → `chapters`；`baseURL` + XPath | 搜索、详情、章节、webview 播放 | `useWebview` 嗅探需宿主 WebView 通道；anti-crawler 仅尽力 |
| **Venera** | `ComicSource` JS 类 | `explore/category/search/comic/chapter` → RSP；`Network/Html/Convert/Storage` → 运行时 shim | 搜索、探索、详情、章节、图片 | `UI.*`、评论 / 登录 webview、部分加密源 |
| **uzVideo** | `WebApiBase` JS（type 101/200/400） | `getClassList/getVideoList/getVideoDetail/getVideoPlayUrl/searchVideo` → RSP | 视频搜索、详情、剧集、播放地址 | 网盘工具（type 300）、加密 `codeID` 源 |

**适配器原则**：只做**接口翻译**，不复制参考项目代码；Venera 为 GPL-3.0，uzVideo 无许可证 ⇒ 采用干净实现。

> **Kazumi 适配的定位（本轮定）**：**只做格式参照，不依赖 Kazumi 本体。**
> 我们只读取规则 JSON 的公开字段并转换成 RSP 声明式规则，不 import / 内嵌 / 修改 Kazumi 的
> Dart / Flutter 代码，不运行它的 XPath 引擎，也不随包分发 `KazumiRules` 的第三方规则。
> 真实规则由用户自行导入，规则许可与合规由用户与规则作者承担。

### 5.7 来源包与分发

- 单来源目录：`<id>/rsp.json` + `rule.json` 或 `index.js` + `assets/`。
- 打包：zip；宿主校验 `id`、路径越界、大小上限、入口存在。
- 索引：`index.json`（列表）+ `index.sig`（Ed25519 签名）+ 每条 `sha256`。
- 更新：来源可独立于应用更新；宿主显示「来源已失效 / 有新版本」。
- 用户自建来源：允许本地导入，标记「未签名」，商店模式下禁用。

### 5.8 规则仓库：聚合 Kazumi / Venera / uzVideo

**目标**：做一份自己的 `registry/index.json`，用统一字段列出三类上游来源；
仓库侧**只保存地址与元数据，不保存规则 / 脚本正文**；插件安装条目时再按
`sourceUrl` 读取，正文只落在本机 `ModuleData`。

```
resource-hub/registry/
  config.json           # 本地配置（从 config.example.json 复制）
  upstream/
    kazumi.json         # 上游索引快照（自备；脚本不联网）
    venera.json
    uzvideo.json
  index.json            # 聚合结果
```

适配器（`packages/rh-source-sdk/src/registry.ts`）：

| 上游 | 输入 | 产出 | 许可处理 |
|---|---|---|---|
| Kazumi | `KazumiRules/index.json` | `engine: declarative`，`sourceUrl` 指向 `<name>.json` | 仓库 MIT：保留 `license` + `attribution` |
| Venera | `venera_comic_source/index.json` | `engine: js`，`sourceUrl` 指向 `.js` | 本体 GPL-3.0；单源许可以上游文件为准，安装时读取、仓库不保存正文 |
| uzVideo | 用户整理的扩展索引 | `engine: js`，`sourceUrl` 指向脚本 | 仓库未见 LICENSE ⇒ 默认保留所有权利；安装时读取、仓库不保存正文 |

合并规则：全局 id 去重、版本高者胜、`warnings` 取并集、按名称排序。

> **「去除声明」的正确边界（必须写清）**：
> 我们做的是**统一字段、去重、去掉重复的元数据**；
> **绝不能删除**上游的版权、许可、作者声明 —— MIT / GPL 都要求保留。
> 也不要把 Venera / uzVideo 的脚本正文搬进本仓库，更不要把它们包装成「自有来源」。
> 插件在用户主动安装时按 `sourceUrl` 读取正文属于运行时行为，不改变仓库的分发边界。
> 如果「去除声明」指的是抹掉第三方归属，那不做。

AI 侧配套：

- `agent-templates/skills/resource-hub/SKILL.md`：教模型写 RSP 规则、跑校验、聚合仓库
  （唯一源；装完即 `<exe 根>\skills\resource-hub\SKILL.md`）。
- `npm run source:validate -- <file>`：RSP / Kazumi / registry 三种校验入口。
- `npm run registry:build`：把 `registry/upstream/*.json` 合并成 `registry/index.json`。

---

## 六、阅读与播放引擎

### 6.1 阅读器外壳（上下框，漫画 / 小说共用）

进入阅读即显示上框与下框，**底色透明度 90%**，3 秒无操作自动隐藏。

| 区域 | 行为 |
|---|---|
| 中心带 | 上下阅读模式取屏幕中间 30%（高度 35%~65%）唤出 / 收起上下框；左右阅读模式取中间 30%（宽度 35%~65%） |
| 上 35% | 上一页（左右模式为左侧 35%） |
| 下 35% | 下一页（左右模式为右侧 35%） |
| 上框 | 退出、书名 / 章节名、目录 |
| 下框 | 最上方可拖动进度条；按钮区：锁定、自动播放、播放速度、下一章、裁剪模式、阅读模式 |

- **锁定**：两框立即收起并进入专注模式，中心点击不再唤出；长按屏幕中心 600ms 解锁。
- **自动播放**：按 `4200ms / 速度` 定时翻页，到末页自动停止并提示。
- **速度**：0.5× / 1× / 1.5× / 2× / 3× 循环。
- **阅读模式**：上下滚动 / 左右分页 / 条漫循环；分区方向随模式切换。左右模式整页适配、
  跨页大图按 `contain` 落在可视区并占满宽度；条漫按章连续拼接，滚动位置回写页码。
- **裁剪模式**：原图 → 页边距 → 裁白边 → 双页拆分 循环。裁白边按像素扫描非白区域
  （跨域读不到像素时保持原图）；双页拆分把宽高比 ≥ 1.2 的大图拆成左右半页，
  翻页时先翻半页。
- **移动端**：上下框 padding 叠加 `env(safe-area-inset-*)`；点击用 Pointer Events，避免 300ms 延迟；
  左右模式支持双指缩放（1–4×）、双击 2× 放大、从顶部下拉退出。
- **PC 滚轮缩放（2026-10-21）**：左右分页模式下滚轮以**指针位置为锚点**缩放（1–4×，
  指针下的内容点保持不动）；已回到 1× 时向下滚轮交还容器默认滚动，向上仍可放大。
  触控板 pinch（`ctrlKey + wheel`）同样生效；缩放状态与双指 / 双击共用同一套
  `zoom / zoomX / zoomY`，仍受 `clampZoomPan` 边界约束。
- **按钮规范**：动作按钮一律「线性 SVG + 文字」，图标 `stroke="currentColor"` 随主题；仅文字按钮尽量不用（规范见 `docs/ui-style.md` §界面 3 / §界面 4）。

### 6.2 漫画阅读器

- 三种模式：**分页**（左右）、**纵向滚动**、**条漫**（连续长图）。
- 预加载：当前页 ±2、下一章前 3 页；失败重试 2 次并显示占位。
- 手势：翻页分区见 §6.1、双指缩放、双击放大、下拉退出；PC 分页模式支持滚轮按指针位置缩放。
- 裁剪：原图 / 页边距 / 裁白边 / 双页拆分；白边检测在 420px 缩略图上扫描，
  结果按 URL 缓存，裁切后用 canvas 原位替换。
- 图片：`<img>` + `loading="lazy"`；需要 Referer 的来源走宿主 / 本地媒体代理。
- 进度：`chapterId + pageIndex`，每 3 秒或翻页时落盘（防写放大）。

### 6.3 小说阅读器

- 格式：纯文本、HTML（清洗白名单）、EPUB（解析 spine；正文 HTML 白名单 + 图片内联为 data URL）。
- 排版：字号 / 行高 / 页宽 / 背景（跟随主题或护眼色）/ 分页或滚动。
- 目录、书签、章节跳转、进度百分比；TTS 为 P2。

### 6.4 视频播放器

| 平台 | 播放路径 | 说明 |
|---|---|---|
| Windows | WebView2 + `hls.js`（MSE） | 大多数 HLS / MP4 可播；MKV 按直链试播，HEVC 等编码提示换线路 / 复制直链（2026-10-29） |
| Android | WebView + `hls.js`（MSE） | 系统 WebView 支持 MSE；后续可接 ExoPlayer |
| iOS | 原生 `<video>`（HLS） | WKWebView 原生 HLS；不走 hls.js；MP4 直连 |
| 兜底 | 外部播放器 / 系统分享 | 反盗链或编码不支持时使用 |

- 字幕：WebVTT 外挂；内嵌字幕 v2。
- 弹幕：P2，接入 DandanPlayer 类开放 API（注意其使用条款）。
- 防盗链：本地媒体代理统一注入 `Referer / User-Agent / Cookie`，并支持 Range。
- **WebView2 的 `canPlayType("application/vnd.apple.mpegurl")` 回 `maybe`，但 Chromium
  并没有 HLS 解复用器**（2026-10-21 实测 Edg/155）：据它走原生分支只会把 `video.src`
  指到代理地址后永远停在 `readyState 0`。判据必须是 `Hls.isSupported()` 优先，
  只有 hls.js 不支持时才回落原生 `<video>`。hls.js 的清单 / 分片超时放宽到 30s ——
  媒体要多过一跳宿主代理，采集站 CDN 常要 10–20s 才吐清单。

### 6.5 本地媒体代理（两种形态都需要）

```
GET http://127.0.0.1:<port>/m/<token>
  → 校验 token（来源 id + 目标 URL + 过期时间 + 允许的 header 集合）
  → reqwest 透传 Range / If-None-Match
  → 返回 206 / 200，附带 Content-Type / Content-Length / Accept-Ranges
```

- token 由宿主生成，**不暴露任意 URL 代理**（防 SSRF）：目标必须是该来源声明过的 host。
- 拒绝私网地址、`file:`、非 http(s) 协议；重定向后再次校验。

---

## 七、Lunac 插件形态

### 7.1 清单

```json
{
  "id": "resource-hub",
  "name": "资源库",
  "description": "漫画 / 小说 / 番剧 / 电视剧聚合阅读与播放",
  "keywords": ["漫画", "小说", "番剧", "电视剧", "资源", "manhua", "anime", "novel"],
  "icon": "📚",
  "version": "0.1.0",
  "entry": "index.js",
  "homepage": "https://github.com/<owner>/resource-hub",
  "permissions": ["window.float", "layout.takeover", "network.request", "media.proxy", "storage.plugin"],
  "window": {
    "width": 1200, "height": 760,
    "minWidth": 720, "minHeight": 480,
    "resizable": true,
    "skipTaskbar": false,
    "alwaysOnTop": false,
    "chrome": true
  }
}
```

- `window.float`：独立悬浮窗打开，主窗回到搜索态。
- `layout.takeover`：内嵌时接管整个结果区，渲染书架 / 阅读器 / 播放器。
- 新权限是**宿主 API v3 的前置条件**，见 7.4。

### 7.2 搜索结果形态

`execute(input)` 返回一段 HTML：

```
┌──────────────────────────────────────┐
│ 📚 资源库                    [打开]   │
│ 《作品 A》 漫画 · 来源甲 · 已完结      │
│ 《作品 B》 番剧 · 来源乙 · 更新至 12   │
│ 搜索 "input" →                         │
└──────────────────────────────────────┘
```

- 只展示前 3 条与「打开资源库」按钮，避免结果区被大块内容挤爆。
- 点击「打开」通过 `attach(root)` 里的事件委托触发宿主打开插件面板。
- 无结果 / 来源全部失败时，显示原因与「管理来源」入口。

### 7.3 主题与交互

- 只用宿主既有变量：`--text / --text-dim / --text-muted / --surface-glass / --border-glass / --accent`。
- 类名统一加 `rh-` 前缀；不写死颜色。
- 动画遵守 `prefers-reduced-motion`；不做大面积 blur（WebView2 掉帧风险）。

### 7.4 宿主能力缺口（必须先补的 API v3）

| 能力 | 现状 | 需要 |
|---|---|---|
| 外网 JSON / HTML | CSP `default-src 'self'`，`fetch` 外网被拦 | `net.request({ url, method, headers, body }) → { status, headers, body }` |
| 图片防盗链 / Cookie | `<img>` 只能直连 `https`，不能带 Referer | `media.proxy(url, headers)` 返回本地 token URL |
| 视频播放 | `media-src` 未放行外部地址 | 本地代理 + CSP 放行 `http://127.0.0.1:<port>` |
| JS 来源 | 无 `eval`（CSP 未开 `unsafe-eval`） | 宿主 Rust 侧 QuickJS：`engine.eval(sourceId, code, input)` |
| 插件持久化 | 只能蹭同源 `localStorage` | `store.get/set/remove/keys`（按插件 id 命名空间） |

- 建议以 **`apiVersion: 3`** 增量提供，旧宿主上插件降级为「仅本地书库」。
- 权限必须在清单声明并在市场面板如实展示；`network.request` 需要 host allowlist。
- **不要**用放开 `unsafe-eval` 的方式跑 JS 源：既破 CSP，也放大供应链风险。

### 7.5 插件内的 v1 / v1.5 切分

| 版本 | 能力 | 依赖 |
|---|---|---|
| v1 | 本地书库、OPDS / WebDAV、声明式来源（Kazumi 规则）、漫画 / 小说阅读、图片直连 | 宿主 API v3 的 `net.request` + `storage.plugin` |
| v1.5 | 视频播放、防盗链图片、下载 | `media.proxy` + CSP 放行 |
| v2 | Venera / uzVideo JS 源 | 宿主 QuickJS `engine.eval` |

---

## 八、独立应用形态（iOS / Android）

> Windows 不做独立应用 —— 它的形态是 Lunac 拓展插件（见第七章）。

### 8.1 技术栈

- **外壳**：Tauri 2（iOS / Android），与 Lunac 同栈，复用 Rust 经验。
- **前端**：同一套 `packages`（无框架 Web Components + CSS token），Vite 构建。
- **Rust 核心**：
  - `reqwest`：HTTP / Cookie / 代理 / 重定向控制；
  - `rquickjs`：JS 来源运行时（full 模式）；
  - `scraper` / `sxd-xpath`：声明式提取；
  - `rusqlite`：书架 / 进度 / 下载 / 缓存；
  - 本地媒体代理（Tauri 自定义协议或 `axum`），支持 Range；
  - 下载队列：分片、断点续传、并发上限。
- **播放**：Android 用 `hls.js`（WebView MSE），iOS 用原生 `<video>`（HLS）；v2 再接 ExoPlayer。

### 8.2 平台差异

| 平台 | 分发 | 备注 |
|---|---|---|
| **iOS** | `full`：自签名 / AltStore；`store`：App Store **Lite** | 自签名需要开发者证书；Lite 禁用 JS 源与来源市场；见 9.2 |
| **Android** | **只发 APK**（官网 / GitHub Release / Obtainium），**本轮不走 Google Play** | 存储用 SAF；后台下载用前台服务；Play Protect 可能提示「未知来源」 |

### 8.3 构建开关（编译期）

```rust
// 只影响能力，不影响 UI 组件复用
#[cfg(feature = "store-mode")]   // 仅 iOS Lite：无 JS 引擎、无来源市场、无 NSFW
#[cfg(feature = "full-mode")]    // iOS 自签名 / Android APK：全部能力
```

- `store-mode` 同时关闭：远程规则导入、JS `eval`、剪贴板导入来源、来源推荐。
- 允许：本地文件、OPDS、WebDAV、用户自有 URL 的**媒体播放**（不下载规则）。
- Android 只构建 `full-mode`；iOS 同时产出 `full`（侧载）与 `store`（Lite）。

### 8.4 版本与发布

- 版本源单一真相源（`packages/core/version.ts`），三端构建读取同一版本。
- 发布物：`ResourceHub-<ver>.apk`、`ResourceHub-<ver>.ipa`（侧载）、AltStore Source（`alt_store.json`）、
  App Store Lite 构建。
- 来源索引与应用解耦：应用发版不带来源，来源走独立仓库 + 签名。
- **无广告、无内购、无遥测**；个人项目、非商用（见 9.6）。

### 8.5 iPad HD 适配

> 状态（2026-10-18）：共享内核 `rh-reader` 已适配；Tauri 移动端外壳尚未接入。
> 手机竖屏 / 矮视口已适配（2026-10-21，见下）。

- **共享内核断点**（`packages/rh-reader/src/reader-ui.ts`）：
  - `≥768px`（iPad 竖屏）：正文列宽 `min(820px, 74ch)`、字号 1.08rem、行高 2；
    上下框左右留白 24px；按钮 ≥40px、进度条加厚。
  - `≥1024px 且横屏`（iPad 横屏 / Stage Manager）：正文列宽 `min(900px, 80ch)`，
    左右留白 72px，避免正文贴边。
  - 图片：非条漫模式限制 `max-height`，整页图不超出可视区；条漫保持连续长图。
- **安全区**：页面与上下框的 padding 全部叠加 `env(safe-area-inset-*)`（含左右），
  适配横屏圆角与分屏；`overscroll-behavior: contain` 防止整页回弹。
- **触控**：`@media (pointer: coarse)` 下按钮 ≥40px、进度条 26px；
  去掉 iOS 点击高亮（`-webkit-tap-highlight-color: transparent`）。
- **外壳要求**（`apps/mobile/` 接入时必须遵守）：
  - `<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">`；
  - iOS 允许竖屏 + 横屏；布局按**容器宽度**自适应，不按固定设备宽度；
  - 安全区依赖 `viewport-fit=cover`，未设置时 `env()` 全为 0。
- **验收**：浏览器以 1024×768（竖屏）与 1366×1024（横屏）模拟，检查正文列宽、
  控件高度、左右安全区与图片完整可见。
- **后续（v1.5）**：横屏双页对开、Apple Pencil 翻页。

#### 8.5.1 手机竖屏 / 矮视口（2026-10-21）

- **断点**：`@media (max-width:640px), (max-height:480px) and (pointer:coarse)`
  —— 竖屏手机与横屏手机（高 375px 这类矮视口）共用一套紧凑规则。
- **上下框**：动作按钮只留图标（文字标签 `.rh-reader-btn-label` 隐藏），
  速度 / 模式保留数值文案（`1×`、`左右分页`）；底部操作栏 `nowrap + space-between`
  单行排布，6 个按钮全部 44×44 起（触屏 `pointer:coarse` 同样 44px）；
  页码与进度条字号收小，标题左对齐。
- **页面留白**：`52px`（上）/ `14px`（左右）/ `112px`（下），全部叠加安全区；
  正文 1.02rem / 行高 1.95；非条漫图片上限改为 `100dvh - 128px`，横屏不再被
  平板断点的 `-196px` 压扁。
- **目录 / 书签**：面板在手机上占 92% 宽（原 86%），条目 12px 内边距便于点按。
- **手势**：左右分页下新增**单指横滑翻页**（阈值 48px、纵向 <40px、600ms 内，
  缩放态不触发）。实现放在 touch 事件而不是 pointerup：Chrome 在拖动开始就会
  发 `pointercancel`，用 pointerup 判断会被吞掉；touch 序列不受影响。
  竖向 / 条漫模式横滑不翻页，点击分区逻辑不变；桌面鼠标不触发横滑。
- **iOS 细节**：stage 增加 `-webkit-touch-callout:none`（长按不出系统菜单），
  与既有 `-webkit-tap-highlight-color:transparent` / `user-select:none` 一致。
- **横竖屏切换**：`matchMedia("(max-width:640px), (max-height:480px) and (pointer:coarse)")`
  的 `change` 事件只重绘上下框文案，不重排页面；`destroy()` 时移除监听。
- **验收（CDP 375×812 / 812×375 / 1280×800）**：竖屏无横向溢出（`scrollWidth=375`）、
  6 个按钮同排 44px、标签全部隐藏、速度 `1×`、模式 `左右分页`、
  页留白 `52/14/112/14`、正文 16.32px；横滑 `1/6 → 2/6`；
  竖向模式横滑不变（`2/6`）；右侧单击仍翻页（`2/6 → 3/6`）；
  桌面恢复 7 个文字标签、按钮 40px、留白 `64/72/132/72`、正文 17.28px。

---

## 九、安全与合规

### 9.1 威胁模型

| 威胁 | 场景 | 对策 |
|---|---|---|
| 恶意来源代码 | 用户导入 JS 源 | QuickJS 隔离、无文件 / 进程 API、超时与内存上限、权限清单 |
| SSRF | 来源请求内网 / 本机端口 | host allowlist + 私网 IP 拒绝 + 重定向复检 |
| HTML 注入 | 来源返回的标题 / 简介 | 全部转义；只允许白名单标签 |
| 凭据泄露 | Cookie / Token | 按来源隔离的 Cookie Jar；系统钥匙串；日志脱敏 |
| 供应链 | 来源包被替换 | Ed25519 签名 + `sha256` + 版本回滚保护 |
| 版权风险 | 分发未授权来源 | `store` 模式不内置、不索引；`full` 模式由用户自行导入 |

### 9.2 iOS App Store 合法发布调研（重点）

#### 9.2.1 相关条款

| 条款 | 内容要点 | 对本项目的影响 |
|---|---|---|
| **2.5.2** | App 必须自包含；不得下载、安装或执行会**引入或改变功能**的代码（解释型代码同样受限） | **远程 JS 源 / 远程规则市场是明确高危**；「下载规则驱动行为」也会被视为越线 |
| **4.7** | 允许 HTML5 / JavaScript 小程序、小游戏、聊天机器人、**插件**；但必须只使用 **WebKit + JavaScriptCore**、不得暴露原生 API、免费或走 IAP、提供年龄分级与限制，且宿主对全部内容负责 | 理论上可承载「插件」，但要求极严；不能借此绕开 2.5.2 的「改变核心功能」 |
| **5.2.1** | 未经许可不得使用受保护的商标、版权作品、专利 | 图标、截图、内置作品封面 / 名称都可能触发 |
| **5.2.2** | 使用 / 访问 / 变现第三方服务内容，必须获得该服务条款的明确许可 | **聚合未授权站点 = 直接命中**；审核可能要求提供授权证明 |
| **3.1.1 / 3.1.3(a)** | 数字内容交易走 IAP；Reader App 可有限外链 | 本项目**无内购、无付费**，不涉及；但也不能用「外部付费」绕过 |
| **1.2** | 用户生成内容需过滤、举报、拉黑、联系方式 | 弹幕 / 评论功能必须配套审核 |
| **4.2** | 最低功能要求，不能只是网站套壳 | 必须有原生阅读器 / 播放器 / 书架 / 下载等实质功能 |

#### 9.2.2 现实证据

- Venera：iOS 通过 **AltStore** 分发（仓库内含 `alt_store.json`），而非 App Store。
- Kazumi：README 明确 iOS「需要自签名」（`kazumi.app/docs/misc/how-to-install-in-ios.html`）。
- 两个项目都不在 App Store 上架，说明这类「来源可插拔聚合器」的审核风险是行业共识，而非个案。

#### 9.2.3 能不能用「远程仓库资源 URL」绕过监测？——**不能，也不建议尝试**

结论：**技术上行不通、规则上明确禁止、后果不可接受**。

1. **2.5.2 的打击对象就是「下载的代码」**：远程 `raw.githubusercontent.com/*.js` / 规则 JSON、
   运行时 `fetch` 再 `eval` / `import()`，都在条款覆盖范围内；「代码在 GitHub 不在 App 包里」不改变
   「App 下载并执行了改变功能的代码」这一事实。
2. **4.7 不是免死金牌**：它只允许受控的 HTML5 / JS 插件，且要求纯 WebKit + JavaScriptCore、不暴露原生 API、
   宿主对全部内容负责；一个「第三方来源市场 + 任意站点抓取」显然超出其边界。
3. **绕审本身就是红线**：Apple 指南前言写明，试图欺骗审核流程（trick the review process）会导致
   **应用下架并被开除开发者账号**。这不是「概率问题」，而是账号级风险。
4. **技术上也会被发现**：审核会检查网络行为、要求演示账号 / 演示路径；上架后还可被抽查、被投诉、
   被自动化扫描；远程仓库地址、CSP 放行、请求特征都可被识别。
5. **即使侥幸上架，版权风险仍在**：5.2.2 / 5.2.1 与真实权利人的投诉不依赖审核机制；
   下架、律师函、账号封禁都可能滞后发生。

**正确做法**：把「来源生态」留在 `full`（自签名 / AltStore / Android APK / Windows 插件），
App Store Lite 只做「本地书库 + 授权来源」，**不在 Lite 里留任何远程代码通道**。
不要用「远程 URL 延迟加载」「审核时关闭开关」「服务端下发规则」这类规避手段 —— 它们属于同一类绕审。

#### 9.2.4 上架账号与前置条件

| 项目 | 要求 | 说明 |
|---|---|---|
| **Apple Developer Program** | **必须**；**99 USD/年**（或本地货币），个人账号即可 | 免费 Apple ID 只能本机调试 / 7 天签名，**不能提交 App Store** |
| **账号类型** | 个人（Individual）或组织（Organization） | 个人账号的**法定姓名会作为卖家名**显示；组织需要 D-U-N-S |
| **App Store Connect** | 免费 App 只需接受 **Free Apps Agreement** | **不需要** Paid Apps Agreement、税务表（W-8BEN 等）、银行账户 —— 因为没有收入 |
| **年龄 / 身份验证** | 需成年、Apple Account 开启双重认证 | 个人开发者通常审核较快 |
| **中国大陆区上架** | 需要 **ICP / App 备案号**（工信部） | 通常在 App Store Connect 提供备案信息；个人主体能否备案、是否需要国内主体 / 服务器，按当地最新规定办理 |
| **隐私** | Privacy Manifest、数据收集声明 | 本项目不收集数据、无广告、无遥测，仍需如实填写 |
| **出口合规** | 加密使用声明 | 使用 HTTPS / 标准加密需按 App Store Connect 问卷申报 |
| **审核素材** | 截图、描述、演示路径 | 描述中不得出现「盗版 / 免费看全网」等表述 |

> 费用口径：会员 **99 USD/年**；免费 App 不产生 Apple 分成，也不涉及税务 / 银行信息。
> 若未来加内购或付费，才需要 Paid Apps Agreement 与税务 / 银行资料（本项目明确不做）。

#### 9.2.5 三种 iOS 路径（本轮已定）

| 路径 | 本轮决定 | 说明 |
|---|---|---|
| **A. App Store Lite** | **暂不做** | 预算不支持 99 USD/年会员；方案与检查表保留，供以后重启 |
| **B. 自签名 / LiveContainer** | **做（唯一 iOS 分发路径）** | 免费 Apple ID 7 天续签、最多 3 个 App；Windows 侧用 Impactor + LiveContainer + SideStore（见 9.2.8） |
| **C. 欧盟替代市场 / 网页分发** | **不做** | 已去掉；仅作长期备选 |

#### 9.2.6 若提交 App Store Lite：最低自查清单

1. `store` 构建中**没有任何**来源市场、远程规则、JS `eval`、剪贴板导入来源。
2. 内置内容只有：本地文件、用户自己的 WebDAV / OPDS、**可出示授权**的来源。
3. 隐私清单（Privacy Manifest）、数据收集声明齐全；不收集遥测、无广告 SDK。
4. 年龄分级：无 NSFW；若有成人内容入口，必须默认关闭 + 年龄门（仍可能被 1.1.4 拒绝）。
5. 举报 / 屏蔽 / 客服入口（若含 UGC）。
6. 审核备注中明确「播放器 / 阅读器不提供内容，来源由用户自行配置」——**但这不能替代授权**。
7. 准备授权证明（若内置任何第三方来源）。
8. 中国大陆区准备 ICP / App 备案号。

#### 9.2.7 结论

> **现阶段不上 App Store。** iOS 只走自签名 / LiveContainer；
> Lite 方案、账号要求与检查表保留在本文档，作为以后预算允许时的重启材料。
> **不允许**用远程仓库 URL、服务端下发规则等方式绕过审核。

#### 9.2.8 Windows 自签名工作流（LiveContainer 的实际用法）

**先厘清一件事**：**LiveContainer 不是签名工具**，而是一个「容器 / 启动器」。
它本身必须先用 AltStore / SideStore 安装并签名；之后它从 AltStore / SideStore **导入开发证书**，
在容器内用内置的 zsign 给其它 IPA 签名（JIT-Less 模式）。所以 Windows 侧要解决的是：

```
Windows 工具（Apple ID 登录）→ 安装 LiveContainer(+SideStore) → 设备内导入证书
→ LiveContainer 添加我们的 unsigned IPA → 启动 / 每 7 天刷新
```

##### 关键事实（决定方案是否可行）

| 事实 | 影响 |
|---|---|
| LiveContainer 需要 **development profile**（含 `get-task-allow`） | **distribution / 企业证书签的 LiveContainer 不能工作**；不要买所谓「企业签名」 |
| 免费 Apple ID = **7 天证书 / 3 个 App / 10 个 App ID** | 需要每 7 天刷新；LiveContainer 只占 1 个槽位，可跑多个 IPA |
| 付费开发者账号（99 USD/年）= **1 年证书** | 不想频繁刷新就买；但 App Store Lite 同样需要这个会员 |
| iOS 15+（多任务 16+） | 老设备不支持 |
| 容器内应用**收不到推送通知**，部分 App 会拒绝运行 | 我们的资源应用无推送需求；但要在真机验证视频 / 下载 / 后台行为 |
| LiveContainer 为 **AGPL-3.0** | 我们只提供使用说明与链接，**不 fork、不随包分发**；若修改它，必须按 AGPL 开源 |

##### Windows 工具对比

| 工具 | 角色 | 优点 | 限制 |
|---|---|---|---|
| **Impactor**（原 PlumeImpactor，跨平台） | 用 Apple ID 签名 + 安装 IPA | GUI；会自动给 LiveContainer 补 entitlement；支持「只注册主 Bundle」；Windows 可用 | 首次需 iTunes 驱动；免费账号 7 天 |
| **SideStore** | 设备端「签名商店」+ 刷新 | 装好后可在手机上刷新，不必每次连 PC；LiveContainer+SideStore 版只占 1 个槽 | 首次仍需 Windows 工具/配对文件；依赖 LocalDevVPN；配对文件可能失效 |
| **Sideloadly** | 单 IPA 直接签名安装 | 最简单：拖 IPA + Apple ID | 不能把证书交给 LiveContainer；3 个 App 限制；7 天 |
| **AltServer / AltStore** | Windows 安装 + 证书导出 | 成熟；可从 AltStore 导出证书给 LiveContainer | 需 iTunes/iCloud（非 Microsoft Store 版）；7 天刷新 |
| **zsign（CLI）** | 纯命令行签名 | 可进 CI 自动签名 | 需要 p12 + mobileprovision；免费账号的 provisioning 不便自动化 |

##### 推荐流程（按场景）

**场景 A：只测我们的 IPA（最快，不折腾容器）**

1. Windows 安装 iTunes（**非 Microsoft Store 版**，提供 Apple 驱动）+ Sideloadly（或 Impactor）。
2. USB 连接 iPhone，信任电脑。
3. 把 `ResourceHub-unsigned.ipa` 拖进 Sideloadly，输入 Apple ID，开始签名安装。
4. iPhone：设置 → 通用 → VPN 与设备管理 → 信任开发者；iOS 16+ 打开「开发者模式」。
5. 7 天后重签一次。适合开发自测，不适合对外分发。

**场景 B：长期使用 / 多版本共存（推荐，LiveContainer + SideStore）**

1. Windows 安装 iTunes（非 Microsoft Store 版）+ **Impactor**。
2. Impactor 登录 Apple ID；拖入 **LiveContainer+SideStore.ipa**；勾选 **Only Register Main Bundle**；安装。
3. iPhone 信任开发者证书；iOS 16+ 打开开发者模式。
4. 打开内置 **SideStore** → 登录同一 Apple ID → 连接 **LocalDevVPN** → `Refresh All`。
5. 打开 **LiveContainer** → 设置 → **从 SideStore 导入证书** → 运行「免 JIT 模式诊断」，确认通过。
6. LiveContainer 右上角 `+` → 选择我们的 **unsigned IPA** → 启动。
7. 每 7 天：SideStore → `Refresh All`（免费账号）；付费账号证书 1 年。

**场景 C：不想每次连 Windows**

- 先用场景 B 装好 SideStore + 配对文件；之后在设备上连 LocalDevVPN 刷新。
- 配对文件可用 **JitterbugPair**（Windows）或 iloader / AltServer 生成；系统更新或重置后需重新配对。

##### 我们的 IPA 怎么产出

- CI 只产出 **unsigned IPA**：`xcodebuild` 关闭签名（`CODE_SIGNING_ALLOWED=NO`）→ 打包 `Payload/ResourceHub.app` → zip 改后缀 `.ipa`。
- **不内置 provisioning profile**，由用户用自己的 Apple ID 签名；这样不绑定设备 UDID，也不泄露证书。
- 若将来要做「已签名 IPA」：只能走 development / ad-hoc（需收集设备 UDID）或 App Store / TestFlight；
  **不要**购买第三方「企业签名」分发，违反 Apple 开发者协议且随时掉签。

##### 本项目要提前验证的点

1. Tauri / WKWebView 在 LiveContainer 内能否正常渲染、播放 HLS、写本地文件。
2. 后台下载 / 后台播放是否被容器限制（不行就退化为前台下载）。
3. 若使用 Share Extension / 文件关联，安装时需勾选 **Keep App Extensions (Use Main Profile)**。
4. LocalDevVPN 与我们的本地媒体代理（127.0.0.1）是否冲突。
5. App Store Lite 构建不要引入 LiveContainer 相关代码或说明（它只属于侧载版文档）。

> **预算提醒**：如果预算是 0，则只能走「免费 Apple ID + 7 天刷新」的侧载路线；
> **App Store Lite 无法上架**（提交必须有 99 USD/年的 Apple Developer Program）。
> 本轮已决定暂不投入 App Store，因此 iOS 只保留自签名路线。

#### 9.2.9 国内网络：自建 Anisette 服务器（个人使用）

**先定位问题**：SideStore 里所谓的「签名服务器」通常指 **Anisette 服务器**。
它伪装成一台 Mac，生成 anisette 数据供 Apple 登录 / 证书流程使用；**它不参与实际签名，也不接触账号密码**
（SideStore 官方文档口径）。国内直连公共 anisette 服务器可能超时或返回坏数据，表现是「登录失败 / 刷新失败 /
签名失败」。这不是 Apple 封号，也不一定要改签名方式。

##### 可行方案：自建 anisette-v3 服务器 + 自定义服务器列表

1. **准备一台国内可访问的服务器**：国内 VPS、家里 NAS / 软路由均可；关键是它能稳定访问 Apple 的
   anisette 相关端点（多数网络可访问，但云厂商 IP 可能被风控 / 限速，需实测）。
2. **跑 anisette-v3-server（Docker）**：

   ```bash
   docker run -d --restart always --name anisette-v3 \
     -p 6969:6969 \
     -v anisette-v3_data:/home/Alcoholic/.config/anisette-v3/lib/ \
     dadoum/anisette-v3-server
   ```

3. **加 HTTPS 反代**：用 Caddy / Nginx 把 `https://ani.example.com` 反代到 `127.0.0.1:6969`。
   SideStore 需要可访问的 URL；不要长期用明文 HTTP。
4. **自建服务器列表 `servers.json`**（只放自己的服务器）：

   ```json
   {
     "servers": [
       { "name": "home", "address": "https://ani.example.com" }
     ]
   }
   ```

   托管在**国内可访问**的位置：自己的服务器、对象存储、Gitee Pages 等。
   **不建议用 GitHub Pages**（国内访问不稳定，且会把问题从 anisette 转移到列表拉取）。
5. **在 SideStore 中切换**：
   - 设置 → 底部 **Anisette Servers**；
   - 把列表 URL 换成你的 `servers.json`；
   - **Refresh Servers** → 选择 `home`。
6. **验证**：重新登录 / 刷新一个 App。若仍失败，按顺序排查：
   - LocalDevVPN 是否连接；
   - 配对文件是否失效（系统更新 / 重置后需重新生成）；
   - iOS 16+ 开发者模式是否打开；
   - Apple ID 是否触发 2FA / 证书吊销；
   - 换回公共 anisette 服务器做 A/B，确认是不是服务器问题。

##### 安全与合规边界

- **只给自己用**：Anisette 服务默认无鉴权，暴露公网会被滥用并抬高账号风控；至少做来源 IP 限制、限速、随机路径。
- **用备用 Apple ID**：不要拿主账号试；v3 协议比 v1 安全，但仍建议隔离。
- **服务器要能访问 Apple**：如果国内 VPS 到 Apple 的链路不稳，可退而选择香港 / 新加坡轻量服务器（成本低、两边都较稳）。
- **这不是绕过签名**：证书仍由 Apple 签发，免费账号仍是 7 天；anisette 只解决「拿不到登录/签名所需数据」。
- **不要公开分发服务器**：个人自用；对外提供等于替陌生人承担账号风控与滥用风险。
- **不要修改 / 二次分发 SideStore 或 LiveContainer**：遵守其开源许可；我们只在文档里给链接与配置方法。

> 若失败点不在 anisette（例如安装阶段报 pairing / AFC 错误），改服务器不会解决；
> 先按 SideStore 的 Common Issues 排查配对文件、LocalDevVPN 与证书状态。

### 9.3 Google Play（本轮不走）

- **本轮决定：不发布 Google Play**，Android 只发 APK（官网 / GitHub Release / Obtainium）。
- 若未来重新评估，需注意两条硬政策：
  - **Intellectual Property**：禁止侵权，也禁止「鼓励或诱导侵权」；DMCA 投诉驱动下架。
  - **Device and Network Abuse**：禁止下载 / 执行未经 Play 分发的**可执行代码**；WebView 中的 JavaScript
    属于解释器内运行，通常被允许，但不能借此改变应用核心功能或规避审核。
- 现实：IPTV / 聚合类应用常因版权投诉被移除；Google 甚至可按司法命令通过 Play Protect 远程卸载。
- ⇒ 保持「`full` 只发 APK」是当前唯一自洽的选择。

### 9.4 Microsoft Store（本轮不上架）

- Windows 的交付形态是 **Lunac 插件**，不产出独立 Windows 应用，因此不存在 Microsoft Store 上架问题。
- 若将来做独立 Windows 版，再单独评估商店政策；当前不投入。

### 9.5 内容治理（所有形态）

- 来源分级：`safe` / `nsfw`；`nsfw` 默认隐藏，需年龄门。
- 来源下架机制：签名索引支持 `revoked: true`，客户端收到后禁用并提示。
- 举报入口：来源级 + 作品级；记录来源 id，不记录用户内容。
- 不托管内容：不做图片 / 视频转存，不做公开分享链接。
- 不内置第三方来源脚本 / 规则；示例来源只使用公共领域或自建测试站点。

### 9.6 许可证与项目声明（本轮定）

- **许可证：MIT**。仓库根放 `LICENSE`（MIT 全文），第三方组件放 `THIRD_PARTY_NOTICES.md`。
- **项目性质**：个人项目、**非商用**、**不出现任何广告**、无内购、无付费墙、无遥测。
- **不内置第三方来源**：Venera / uzVideo / Kazumi 的源脚本与规则**不随包分发**，由用户自行导入；
  我们只实现协议与适配器，**不复制**参考项目代码。
- **许可兼容性**：
  - Venera、Kazumi 为 **GPL-3.0**。MIT 项目**不能**直接链接 / 复制它们的代码；适配器必须**干净实现**
    （只读公开文档与协议、独立编写）。
  - uzVideo 仓库**未见 LICENSE**（默认保留所有权利）⇒ 同样只做接口级兼容，不复制代码。
  - 用户导入的第三方来源脚本，其许可由来源作者决定；应用只作为运行器，不重新分发。
- **声明落点**：README、App Store 描述、应用「关于」页统一写「个人项目 · 非商用 · 无广告 · 不提供内容」。

---

## 十、里程碑与验收

> 只列阶段与退出条件，不排时间。

| 阶段 | 交付 | 退出条件 |
|---|---|---|
| **M0 协议** | RSP v1 类型、清单校验、DSL 解释器、测试夹具 | 10 个静态 HTML 夹具全部解析通过；错误码齐全 |
| **M1 插件 MVP** | Lunac 插件外壳、本地书库、声明式来源、漫画阅读 | 搜索 → 详情 → 章节 → 阅读全链路可用；进度重启不丢 |
| **M2 多来源** | Kazumi 适配器、聚合搜索、去重、书架 | 20 条真实规则中 ≥70% 可搜索 / 可读；失败来源可见原因 |
| **M3 小说** | 小说阅读器（TXT / HTML / EPUB）、目录 / 书签 / 进度 | 三种格式可读；排版与进度重启不丢 |
| **M4 宿主 API v3** | `net.request` / `media.proxy` / `storage.plugin` / `engine.eval` | 插件可拉外网 JSON / 带 Referer 的图片 / 视频；旧宿主优雅降级 |
| **M5 视频** | HLS / MP4 播放、字幕、外部播放器兜底 | 插件与独立应用共用播放器；HLS / MP4 各 3 个源可播 |
| **M6 iOS / Android** | APK（不上 Play）、侧载 IPA、AltStore Source、App Store Lite | APK 可安装；侧载可运行；Lite 提交材料齐全（不保证过审） |
| **M7 生态** | 来源签名、本地导入 / 市场、失效上报、Venera / uzVideo 适配 | 签名校验、撤销、限速、QuickJS 隔离全部有测试 |

### 验收口径（硬）

- 任何来源请求**不得**直连私网 / 本机端口。
- 任何来源返回的文本进入 DOM 前必须转义。
- 来源失败必须显示原因，不得静默空列表。
- `store` 构建**不得**包含 JS 执行入口与来源市场代码路径（构建期裁剪，不是运行时开关）。
- 阅读进度、书架、下载记录在崩溃后不丢（WAL / 原子写）。

---

## 十一、风险与待拍板

### 11.1 主要风险

| 风险 | 说明 | 缓解 |
|---|---|---|
| **宿主 API 缺口** | Lunac 插件当前无法外网 JSON / 媒体 | 先落 API v3；或插件 v1 只做本地 + 声明式 |
| **Tauri 移动端视频** | iOS WKWebView 不支持 MSE；Android 编码兼容性 | iOS 原生 HLS；Android `hls.js`；v2 接原生播放器 |
| **来源生态维护** | 站点改版导致大量来源失效 | 来源与应用解耦、失效上报、适配器热更新 |
| **版权与下架** | 商店投诉、域名封锁 | 双模式、用户自导入、来源撤销、法务评估 |
| **许可污染** | Venera / Kazumi 为 GPL-3.0；uzVideo 无许可证 | 干净实现适配器；不复制代码；保留 NOTICES |
| **UI 体量** | 阅读器 + 播放器 + 书架规模大 | 无框架 Web Components；先做 P0，P1 分期 |
| **插件单文件体积** | 所有依赖必须内联 | 避免重型框架；按需裁剪；gzip 后目标 < 1.5MB |

### 11.2 已拍板（2026-10-18）

| 事项 | 决定 |
|---|---|
| Windows 形态 | **只做 Lunac 拓展插件**，不做独立 Windows 应用 |
| 首发形态 | **Windows 插件优先**；iOS / Android 独立应用随后共享内核 |
| iOS 策略 | **仅自签名**；**App Store 暂不考虑**（Lite 方案留档） |
| App Store 账号 | **本轮不购买**（免费 Apple ID 自签名）；Lite 与 99 USD/年会员方案留档 |
| iOS 自签名（Windows） | **Impactor 安装 LiveContainer+SideStore → 导入证书 → 添加 unsigned IPA**；单 IPA 快速测试用 Sideloadly |
| 国内网络 | **自建 Anisette v3 服务器 + 自定义 servers.json**（见 9.2.9） |
| 远程 URL 绕审 | **不做**；遵守 2.5.2 / 4.7 / 5.2，Lite 不留远程代码通道 |
| Android | **暂不走 Google Play**，只发 APK |
| 许可证 | **MIT** |
| 项目声明 | 个人项目、非商用、无广告、无内购、无遥测、不提供内容 |
| 来源内置 | **不内置第三方来源脚本 / 规则**，由用户自行导入 |
| 宿主 API v3 | **接受为前置工作**（`net.request` / `media.proxy` / `storage.plugin` / `engine.eval`） |

### 11.3 仍需确认

1. **产品名 / 品牌**：暂用 `resource-hub`（中文「资源库」），是否另起名？
2. **来源优先级**：Kazumi 声明式 → Venera JS → uzVideo JS，是否按此顺序？
3. **首发范围**：Windows 插件 v1 是否只做漫画 + 声明式来源（小说 / 视频放 v1.5）？
4. **来源索引托管**：自建 GitHub 仓库并签名，还是先只支持本地导入？

---

## 十二、并行开发计划（AI 侧 / 插件侧）

> 目标：**两条线同时开工、互不阻塞**。前提是先冻结接口，再用 mock 各写各的。

### 12.1 契约先行（Week 0，必须先做完）

1. **冻结 RSP v1**：`packages/rh-source-sdk/src/contract.ts`（Work / Chapter / Page / Playback / 错误码）。
2. **冻结宿主 API v3**：`net.request` / `media.proxy` / `storage.plugin` / `engine.eval` 的 TS 类型与返回形状。
3. **mock 宿主**：`packages/rh-source-sdk/src/mock-host.ts`，让插件侧在没有 Rust 实现时也能跑通全链路。
4. **契约测试**：同一组 fixture 同时跑 mock 与真实宿主；接口变更必须升 `apiVersion`。

### 12.2 Track A：AI / 宿主侧

| 编号 | 任务 | 产出 | 验收 |
|---|---|---|---|
| A1 | 宿主网络层 | Rust `net_request`（GET/POST、headers、cookie、超时、大小上限） | SSRF / 私网拒绝 / 重定向复检测试 |
| A2 | 媒体代理 | `media_proxy`（Range、Referer/UA/Cookie 注入、token、host allowlist） | 视频拖动、图片防盗链可用 | **已实现（2026-10-18）**：`lunac-media` 自定义协议 + `permissions: ["media.proxy"]`；转发 Range、拒绝私网、重定向逐跳复检。**Cookie 注入 2026-10-29 补齐**（`&c=` → 「Cookie」头，见 §13.5） |
| A3 | 插件存储 | `plugin_kv`（按插件 id 命名空间、配额） | 卸载插件即清除 | **已实现（2026-10-18）**：`plugin_kv.rs` + `storage.plugin` 能力门；`ModuleData\plugin-data\<id>\kv.json`，原子写 / 坏文件隔离 / 配额，卸载即清 |
| A4 | JS 引擎 | `engine_eval`（rquickjs，超时 / 内存上限 / 无文件系统） | 恶意脚本测试不越权 | **已实现（2026-10-18）**：`plugin_engine.rs` + `permissions: ["engine.eval"]`；一次性 QuickJS 运行时、无宿主绑定、超时中断、32 MiB 内存上限。**JS 源运行时**：沙箱注入 `host.request`（复用 `net.fetch` 能力门与私网拒绝）、补 `Promise` 任务泵，支持 `await`。**Venera 兼容层**：`class ... extends ComicSource` 脚本经沙箱 shim 跑通搜索 / 章节 / 图片。**uzVideo 兼容层**：type 101 显式函数扩展经 `req` / `cheerio` shim 跑通搜索 / 剧集 / 播放 |
| A5 | AI 工具 | `resource_hub_search` / `source_validate` / `source_install` / `source_diagnose` | 模型可写规则、跑 fixture、报错可读 | **已实现（2026-10-18）**：`ResourceHubSearch` / `SourceValidate` / `SourceInstall` / `SourceDiagnose`；安装与插件共用 `<exe 根>\ModuleData\resource-hub\sources` |
| A6 | AI 技能 | `skills/resource-hub/SKILL.md`：教模型写 RSP 声明式规则与排错 | 模型按文档生成可运行来源 | **已实现（2026-10-28）**：源 = `agent-templates/skills/resource-hub/SKILL.md`（唯一来源，随包铺到 `skills\resource-hub\`）。旧稿缺 frontmatter（技能清单里描述为空、等于不可发现）且要求模型去读仓库 `docs/`，装完的环境里根本没有 ⇒ 本次补 `name` / `description`（109 字）并把正文改成自包含：六个工具（含 A7 的 `ResourceHubOpen`）、与执行器对齐的字段名（`title` / `id`·`url` / `cover` / `content` / `playUrl` …）、占位符表、错误码排错与硬边界 |
| A7 | 对话入口 | 自然语言 → 搜索 → 结果卡 → 打开插件 | 主窗与插件窗状态不打架 | **已实现（2026-10-27）**：AI 工具 `ResourceHubOpen`（纯数据，不联网 / 不开窗）→ 聊天出「在阅读里搜索 “X”」卡 → 点卡由宿主 `open_plugin_window` 开阅读窗并把 `{action:"search",query,kind}` 交给插件，插件挂载后落进搜索框并自动搜索（`applySearch`，复用 `paint()+commitSearch()`）。**二期作品级结果卡已实现（2026-10-28）**：插件搜完把前 8 条作品经 `resource_hub_publish_results` 广播回聊天卡（`lunac-resource-hub-results`），点条目带 `{action:"open",work}` 直接进该作品详情 —— 见 §13.5「A7 二期」 |

### 12.3 Track B：插件侧

| 编号 | 任务 | 产出 | 验收 |
|---|---|---|---|
| B1 | 核心与 DSL | `rh-core` + 声明式解释器（CSS/XPath/JSONPath/regex） | 10 个 HTML fixture 全过 |
| B2 | 插件外壳 | `execute` / `attach`、结果卡、悬浮窗、主题变量 | 搜索可命中、面板可开 |
| B3 | 本地书库 | 书架 / 历史 / 进度 / 导入导出 | 重启不丢 | **已实现（2026-10-28 补齐导入导出）**：书架 / 历史 / 进度 / 书签均已落 `localStorage`（重启不丢）；本次补数据包导出 / 合并导入，见 §13.5「B3 书架数据包」 |
| B4 | 漫画阅读器 | 分页 / 纵向 / 条漫、预加载、缩放 | 三种模式可用 |
| B5 | Kazumi 适配 | 规则导入 + 搜索 / 章节 / 图片 | 20 条规则 ≥70% 可用 |
| B6 | 小说阅读器 | TXT / HTML / EPUB、排版、书签 | **已实现（2026-10-18）**：纯文本 / HTML（`htmlToText`）/ EPUB（`parseEpub` 按 spine 展开）；书签（添加 / 列表 / 跳转 / 删除 / 重启保留）已接入 |
| B7 | 视频播放器 | HLS / MP4、字幕、外部播放器 | **已实现（2026-10-18）**：`player.ts`（hls.js + 原生 `<video>`、多线路、字幕、5s 节流回写）；HLS 自定义 loader 把清单 / 分片 / 密钥 / 字幕全部改写为宿主代理地址 |
| B8 | JS ABI 兼容 | Venera `ComicSource` / uzVideo `WebApiBase` → RSP | **已实现（2026-10-18）**：Venera 沙箱 shim 提供 `Network` / `HtmlDocument` / `ComicDetails`；uzVideo type 101 提供 `req` / `cheerio` / `UZUtils` + **扩展数据类 `RepVideo*` / `VideoDetail` / `UZArgs`（2026-10-28，真实脚本跑通）** |

### 12.4 并行节奏与合并点

| 阶段 | Track A | Track B | 合并判据 |
|---|---|---|---|
| P0 | 冻结契约 + mock | 对 mock 写核心 / UI | 契约测试绿 |
| P1 | A1 `net.request` | B1/B2 本地 + 声明式 | 真实网络替换 mock，fixture 不变 |
| P2 | A2 `media.proxy` | B4/B6 阅读器 | 图片防盗链 + 视频可播 |
| P3 | A5/A6 AI 工具 / 技能 | B5 Kazumi 适配 | 模型能生成并验证来源 |
| P4 | A4 `engine.eval` | Venera / uzVideo 适配 | JS 源在沙箱内跑通（Venera / uzVideo 均已完成） |

- **分支**：`feat/host-api-v3`（A）与 `feat/resource-hub`（B），每日对 `main` 合并；接口只改契约文件。
- **功能开关**：`apiVersion < 3` 时插件降级为「仅本地书库」，不得白屏或报错。
- **共享资产**：fixture、错误码、i18n key、设计 token 放 `packages/rh-source-sdk` / `rh-ui`，两边只读。

### 12.5 并行开发的风险控制

1. **AI 生成的来源不可信**：必须过 `source_validate` + fixture + 用户确认，才能安装。
2. **宿主 API 膨胀**：只加这 4 个能力；每个都要有权限名、超时、配额与审计日志。
3. **两边对「来源失败」口径不一致**：统一错误码（network / parse / auth / blocked / unknown），UI 与 AI 都按同一套展示。
4. **并行导致的回归**：契约测试进 CI；插件侧禁止直接 `invoke()` 未声明的宿主命令。

---

## 十三、代码落地进度（2026-10-18 起）

> 代码在仓库根目录 `resource-hub/`（独立工程，后续可整体拆出）。

### 13.1 已完成（M1 起步）

| 模块 | 文件 | 说明 |
|---|---|---|
| RSP v1 契约 | `resource-hub/packages/rh-source-sdk/src/contract.ts` | Work / Chapter / Page / Playback / 错误码 |
| 清单校验 | `.../src/manifest.ts` | id / kind / engine / entry / capabilities 静态校验 |
| 漫画规则转换 | `.../src/manga.ts` | XPath 与 API 两种搜索模式；不支持字段进 `warnings` |
| 规则仓库 | `.../src/registry.ts` | 三源索引适配、去重、版本合并、`parseRegistry` |
| 宿主 API v3 契约 | `.../src/host.ts` | `net.request` / `media.proxy` / `storage.plugin` / `engine.eval`；版本探测与降级 |
| mock 宿主 | `.../src/mock-host.ts` | fetch、内存存储、可选 eval；插件与测试共用同一条链 |
| 规则求值 | `.../src/evaluate.ts` | 模板 / 正则 / JSONPath 子集 / CSS / XPath / 列表抽取 |
| 来源执行器 | `.../src/executor.ts` | 规则 + 宿主 → `ResourceSource`（search / chapters / pages / content / play） |
| 来源运行时 | `resource-hub/apps/lunac-plugin/src/runtime.ts` | 宿主 v3 / mock 选择；多来源并行搜索，单源失败可读 |
| 宿主联网 | `app/src-tauri/src/plugin_net.rs` + 插件悬浮窗桥 | `net.fetch` 能力校验、私网拒绝、超时 / 体积上限；`apiVersion 3` |
| 插件存储 | `app/src-tauri/src/plugin_kv.rs` + `__lunac_host.storage` | 按插件 id 隔离的宿主 KV；`storage.plugin` 能力门；原子写 / 配额 / 卸载即清；旧 localStorage 一次性迁移 |
| 脚本沙箱 | `app/src-tauri/src/plugin_engine.rs` + `__lunac_host.engine.eval` | 一次性 QuickJS 运行时；`engine.eval` 能力门；无文件 / 网络 / 进程绑定；超时中断 + 内存 / 栈 / 输入输出上限 |
| 媒体代理 | `app/src-tauri/src/plugin_media.rs` + `__lunac_host.media.proxy` | `lunac-media` 协议；Range / Referer / UA；只服务声明 `media.proxy` 的插件 |
| 来源执行接入 | `.../src/app.ts` | 导入 `{manifest, rule}` → 搜索 → 详情 → 章节 → 漫画 / 小说阅读、进度落盘 |
| 开发夹具 | `.../test/fixtures/*` + `?fixture=1` | 同源 JSON / SVG，跑通完整链路 |
| 单元测试 | `.../test/{kazumi,registry,evaluate,host,executor,net-client}.test.ts` | 38 条：转换、聚合、JSONPath、mock 宿主、缓存 / 限速 / 取消、执行器映射与错误码 |
| 聚合脚本 | `resource-hub/scripts/build-registry.mjs` | `registry/upstream/*.json` → `registry/index.json`；含本仓库自建 `registry/sources/*.json` |
| 规则仓库 | `resource-hub/registry/index.json` | 59 条元数据（Kazumi 17 + Venera 41 + 自建 1）；自建条目内嵌规则，插件可一键安装 |
| 校验脚本 | `resource-hub/scripts/validate-source.mjs` | RSP / Kazumi / registry 三种输入 |
| AI 技能 | `agent-templates/skills/resource-hub/SKILL.md` | 写规则、跑校验、聚合仓库、许可边界；唯一源，随包铺到 `skills\` |
| 插件外壳 | `resource-hub/apps/lunac-plugin/` | `execute` / `attach`、悬浮窗、搜索 / 书架 / 来源导入（Kazumi 规则或 registry bundle）/ 阅读器接入；动作按钮统一图标 + 文字 |
| 阅读器状态机 | `resource-hub/packages/rh-reader/src/controller.ts` | 章节 / 页 / 模式 / 速度 / 锁定 / 自动播放 / 裁剪；35-30-35 分区纯函数 |
| 阅读器 UI | `.../src/reader-ui.ts` | 上下框（90% 底色、3 秒自动隐藏）、进度条拖动、目录抽屉、长按解锁、`env(safe-area-inset-*)`；按钮均图标 + 文字 |
| 阅读器测试 | `.../test/controller.test.ts` | 8 条：分区、跨章翻页、锁定、循环、进度映射、越界 |
| 插件清单 | `.../public/lunac-plugin.json` | `window.float`，1100×720 |
| AI 工具（A5） | `core-agent/src/resource_hub_tools.rs` | `ResourceHubSearch` / `SourceValidate` / `SourceInstall` / `SourceDiagnose`；只在宿主桥可用时注册 |
| 来源存储 | `app/src-tauri/src/resource_hub.rs` | `<exe 根>\ModuleData\resource-hub\sources\<id>.json`；单条 ≤1 MiB、≤200 条、原子写 |
| 宿主桥方法 | `app/src-tauri/src/mcp_server.rs` | `lunac/resource_hub_sources` / `lunac/resource_hub_install`（删除只留在插件 UI） |

### 13.2 验证

```bash
cd resource-hub
npm install
npm run build      # SDK + reader + 插件（单文件 ESM，约 76KB / gzip 22KB）
npm test           # 38 passed（SDK 30 + reader 8）
npm run typecheck  # exit 0
# 规则仓库 / 单文件校验
npm run registry:build
npm run source:validate -- path/to/rule.json
```

仓库根 `npm run verify` 通过（仅既有 `.ps1` BOM 归一提醒）。

浏览器 harness 已实测：搜索 → 详情 → 收藏 → 阅读 → 书架 → Kazumi 规则导入 → registry 导入
（3 条：kazumi / venera / uzvideo，含许可与 attribution 展示），**控制台无 error**。

阅读器 harness 已实测 13 步：双框初始显示 → 3 秒自动隐藏 → 中心点击唤出 → 上 / 下 35% 翻页 →
左右模式分区 → 锁定收起双框 → 长按中心解锁 → 自动播放 → 速度循环 → 目录跳章 → 退出，
**控制台无 error**。

声明式来源夹具（`?fixture=1`）已实测：导入 `{manifest, rule}` → 多来源搜索 → 详情 → 章节 →
漫画图片阅读（PAGE 1 → PAGE 2）→ 进度落盘；双框自动隐藏 / 中心唤出正常。
控制台仅剩测试环境里一条刻意指向 `example.com` 的残留来源请求失败，属预期错误展示。

### 13.3 尚未接入

- **已接入（2026-10-18）**：宿主 API v3 四项能力、来源缓存 / 限速 / `AbortSignal`、
  视频播放、跨章按需加载、Venera / uzVideo type 101 适配、阅读器书签、iPad HD 共享内核。
- **已接入（2026-10-19）**：左右分页整页适配与跨页大图、条漫连续拼接、裁剪模式
  （页边距 / 裁白边 / 双页拆分）、双指缩放 / 双击放大 / 下拉退出、EPUB 图片与语义样式。
- **已验收（2026-10-21）**：CMS 采集源播放链路（hls.js 优先 + 30s 超时）、
  换源抽屉（同类型来源 + 15s 上限 + 边搜边出结果）—— 见 §测试 13.7。
- **已接入（2026-10-21）**：搜索 / 总览**真分页** —— 每来源按 `Paged.next` 翻页，
  滚到底部联网取下一页并按 `workKey` 去重追加。
- **已放弃（用户裁定，2026-10-21）**：下载 / 离线缓存；多端同步（WebDAV / 跨设备续播）、
  NSFW 开关与年龄门、Venera 账号 / 收藏 / 分类 —— 不再排期，
  设计与待办里出现时按「明确不做」处理。
- **移动端 Tauri 外壳（2026-10-22 暂时移除待办）**：`apps/mobile/` 不接，
  iOS / Android 真机验收一并不排期；共享内核 `rh-reader` 的 viewport 安全区与断点
  适配保留（公开仓库 `lunac-reader-mobile` 仍在），需要时可直接恢复这条待办。
- **视频**：MKV 按直链交给 `<video>` 试播（H.264 / AAC 的 MKV 多数能播）；解不了的编码
  （MKV 里的 HEVC / AC3 等）提示「换线路 / 换源」并可复制直链；**「有声音没画面」也有兜底**
  （8s 内一帧都没出即判定视频轨编码不支持，见 §13.5）。**不接外部播放器、也不内置
  解码器**（抽样 1.5 万条播放地址里 0 条 `.mkv`，属长尾；2026-10-29 见 §13.5）。
- **兼容层**：uzVideo **sniffer 已接（2026-10-28）**、**扩展数据类已接（2026-10-28）**、
  **`Crypto` 已接（2026-10-29）** —— Crypto 就是上游的 CryptoJS，已实现扩展实际用到的子集
  （MD5 / SHA1 / SHA256 / SHA512 / HmacSHA256 / AES CBC·ECB + Pkcs7·NoPadding，见 §13.5）。
- **已从待办删除、不再跟踪**：BaseCode1 嗅探模板、type 300（网盘工具）、type 400（弹幕）、
  加密 `codeID` 源、EPUB 外部 CSS。其中 BaseCode1 与加密源的「导入即拦」是**已有行为**
  （避免装了才发现不支持），不是待办，保留不动。
- **合规边界（明确不做）**：Cloudflare / 站点 JS 挑战的 WebView 绕过。它属于替用户通过站点的
  访问控制，会触及 Cloudflare 服务条款与目标站点条款；命中时按 `blocked` 如实报错即可。

### 13.4 安装到 Lunac（本地验证）

把 `resource-hub/apps/lunac-plugin/dist/` 整个复制到 `<exe 根>\Modules\resource-hub\`，
在「设置 → 插件 → 重新扫描」后搜索「资源库」。演示数据离线可用；导入声明式来源后
会按规则向来源站点发起请求（宿主 v3 未实现时走 mock fetch）。

来源也可以由 AI 安装：模型用 `SourceInstall` 写入
`<exe 根>\ModuleData\resource-hub\sources\<id>.json`，插件下次打开时自动读入；
手动导入仍写同一目录（`resource_hub_install_source` 命令），两条路不会分叉。

### 13.5 进度记录（2026-10-18）

- **UI**：动作按钮统一「线性 SVG + 文字」；已覆盖插件 Tab、搜索、打开、返回、收藏、导入、移除，
  以及阅读器目录关闭。图标只走 `currentColor`，规范落在 `ui-style.md` §界面 3 / §界面 4。
- **来源执行**：新增 `host.ts` / `mock-host.ts` / `evaluate.ts` / `executor.ts`；
  插件 `runtime.ts` 自动选择宿主 v3 或 mock，多来源并行搜索、单源失败可读；
  导入支持 `{manifest, rule}`，打通搜索 → 详情 → 章节 → 漫画 / 小说阅读 → 进度落盘。
- **验证**：SDK 30 + reader 8 = **38 条测试**；`typecheck` / `build` 绿；
  插件产物 **75.75 KB / gzip 21.91 KB**；`harness.html?fixture=1` 实测搜索 → 详情 → 章节 →
  PAGE 1 → PAGE 2，双框自动隐藏 / 中心唤出正常。
- **AI 侧 A5（2026-10-18）**：`ResourceHubSearch` / `SourceValidate` / `SourceInstall` /
  `SourceDiagnose` 四个工具已接进 agent（只在宿主桥可用时注册）；来源写入
  `<exe 根>\ModuleData\resource-hub\sources\<id>.json`，插件启动时读入并用 TS SDK
  再校验一次。`SourceInstall` 与 `SourceDiagnose` 都要用户审批；删除只留在插件 UI。
  **如实记**：`SourceValidate` 是**写入前形状校验**，RSP 语义校验仍只在 TS SDK；
  `SourceDiagnose` 只验证「请求能通 + 内容形态」，完整选择器执行仍在插件 harness。
- **宿主媒体代理 A2（2026-10-18）**：新增 `lunac-media` 自定义协议
  （`plugin_media.rs`），`<img>` / `<video>` 可直接使用；转发 `Range`、注入
  Referer / UA、拒绝私网目标、重定向逐跳复检，单次 ≤32 MiB。
  能力门 = 清单 `permissions: ["media.proxy"]`；资源库清单已声明。
  插件侧只对漫画页 URL 走代理，没有宿主时原样返回（浏览器 harness 不受影响）。
- **宿主插件 KV A3（2026-10-18）**：新增 `plugin_kv.rs`（`ModuleData\plugin-data\<id>\kv.json`），
  `__lunac_host.storage` 从 localStorage 切到宿主命令；按 id 隔离、原子写、坏文件隔离，
  配额 = 键 ≤128 字符 / 单值 ≤256 KiB / 总量 ≤2 MiB / ≤500 键；能力门 =
  `permissions: ["storage.plugin"]`，卸载插件即清数据；旧 localStorage 数据一次性迁移。
- **宿主脚本沙箱 A4（2026-10-18）**：新增 `plugin_engine.rs`（`rquickjs` 0.14），
  `__lunac_host.engine.eval(code, { timeoutMs, args })`；每次调用新建 QuickJS 运行时，
  不注入任何文件 / 网络 / 进程绑定；超时靠 interrupt 中断（默认 3s、上限 10s），
  内存 32 MiB、栈 1 MiB、源码 / 结果各 ≤1 MiB；能力门 = `permissions: ["engine.eval"]`。
  **边界如实**：这是脚本执行边界，不是 OS 沙箱，QuickJS 自身漏洞仍可能突破。
  **仅服务 Windows 插件**；APK / IPA 是独立应用，各自内嵌引擎，只共享 `EngineApi` 契约。
- **插件 UI 重构（2026-10-18）**：插件显示名改为「阅读」；底部纯图标导航
  （总览 / 书架 / 来源）；总览改为封面网格（筛选与排序的最终形态见下一节「二次优化」）；
  书架拆成「喜欢 / 历史记录」两个图标子页，封面 + 名称网格展示；来源页收敛为
  「来源管理 + 导入规则」，导入改为 URL 输入（默认独立公开规则仓库）；
  独立窗默认 1280×720，与 AI 插件对齐。
- **规则仓库导入（2026-10-18）**：`registry/index.json` 已生成 59 条（Kazumi 17 +
  Venera 41 + 自建 1），并独立发布为 `LythrumMoon/lunac-rules`。插件「来源」页可直接粘贴导入；自建 `manual` 条目允许内嵌
  `source`（本仓库自有规则），因此可一键安装后立即搜索；上游条目按 `sourceUrl`
  在安装时读取（Kazumi 转声明式规则，Venera / uzVideo 存脚本正文到本机），
  仓库仍不保存上游正文。演示来源 `demo.gutendex` 使用 Gutendex / Project Gutenberg 公版
  元数据与摘要，验证了「导入 registry → 安装 → 搜索 → 章节 → 阅读」全链路。
- **来源网络客户端（2026-10-18）**：新增 `packages/rh-source-sdk/src/net-client.ts`
  （`createNetClient`）—— GET / HEAD 成功响应 TTL 30s + LRU 64 条；清单 `rateLimit`
  按来源实例做令牌桶；所有来源方法接受 `{ signal, timeoutMs }`，默认 8s 超时；
  取消在排队 / 请求前 / 请求中三处生效。插件运行时为每个来源复用一份客户端，
  宿主对象变化即整表失效。搜索改为「新搜索 / 清空输入即 abort 上一轮」。
- **插件 UI 二次优化（2026-10-18）**：
  - **滚动条**：沿用 `styles.css` 全局唯一 `::-webkit-scrollbar`（4px、轨道透明、
    滑块 `rgba(var(--ink-rgb), .18)`、hover `.32`）；插件内不按容器另写，
    也不写 `scrollbar-width` / `scrollbar-color`。开发 harness 同步同一配方。
  - **总览**：演示与远程结果合并为封面网格（与书架一致），卡片显示封面、标题、
    类型 / 来源 / 状态；去掉行内「打开」按钮，整卡可点进入详情。
  - **筛选抽屉**：参照 AI 历史抽屉（38.2% 宽、玻璃底 + `blur(16px)`、
    `translateX(-100%) → 0`、`0.25s cubic-bezier(.4,0,.2,1)`、遮罩）；
    分页为「来源 / 标签 / 完结·连载」，三页均支持多选，草稿态在点「确定筛选」后
    才生效；支持「清空」、Esc 与遮罩关闭、按钮角标显示已选条件数。
  - **排序**：横向筛选 chips 移除，仅保留「更新时间 / 完结度 / 级数」；
    级数对远程来源使用已加载章节数（未加载时记 0）。
  - **搜索按钮**：改为纯图标（保留 `title` / `aria-label`），不再显示「搜索」文字。
  - **详情页**：左侧展示当前资源封面；新增「播放」按钮（记忆播放），
    从 `state.progress` 恢复章节与页；远程来源阅读会话新增 `baseIndex`，
    把阅读器内部章节序号映射回作品真实章节序号，修复远程进度总记为 0 的问题。
  - **验收**：`typecheck` / 38 + 8 条测试 / `build` 全绿；浏览器 harness 逐步验证
    首屏、抽屉多选筛选、清空、详情、播放、书架与历史记录，无横向溢出。
- **公开规则仓库（2026-10-18）**：规则仓库独立发布为
  [LythrumMoon/lunac-rules](https://github.com/LythrumMoon/lunac-rules)（public）。
  插件默认导入地址改为
  `https://cdn.jsdelivr.net/gh/LythrumMoon/lunac-rules@main/index.json`，
  备选 `https://raw.githubusercontent.com/LythrumMoon/lunac-rules/main/index.json`；
  两者均已验证 HTTP 200（35,093 bytes）。仓库只发布索引与自建来源，不复制上游正文。
- **移动端公开仓库（2026-10-18）**：移动端应用独立发布为
  [LythrumMoon/lunac-reader-mobile](https://github.com/LythrumMoon/lunac-reader-mobile)（public）。
  首批发布共享内核 `packages/rh-source-sdk` + `packages/rh-reader`，明确不依赖
  Lunac 宿主；干净 checkout 下 `npm run build` / `npm test` 通过（38 + 8 条）。
  Tauri 2 移动外壳（Android / iOS 侧载）尚未接入。
- **跨章按需加载（2026-10-18）**：
  - `rh-reader`：`ReaderChapter.lazy` + `ReaderOptions.loadChapter` +
    `ReaderController.replaceChapter`。目录跳转、上一章 / 下一章、自动播放跨章时
    才请求章节内容；同章并发请求复用同一个 Promise；失败显示可读 toast；
    跨章回退会落到新章末页，首章 lazy 时支持恢复指定页。
  - 插件：远程来源进入阅读器时挂**完整章节表**（当前章预载，其余为 lazy 占位），
    `loadRemoteReaderChapter` 按 `${workKey}#${index}` 缓存；进度使用完整章节索引，
    `baseIndex` 固定 0。一次只请求当前章，不再预取整本。
  - 验证：reader 测试 8 → **9 条**（新增 `replaceChapter` 用例），SDK 38 条；
    `typecheck` / `build` 绿。fixture 两章实测：第 2 话首次进入请求
    `demo-pages-2.json`，目录回跳第 1 话命中缓存，上一页跨章落到第 1 话末页。
  - 移动端公开仓库已同步本次 `rh-reader` 变更（`72d3b74`）。
- **公开口径收敛（2026-10-18）**：
  - `lunac-reader-mobile/README.md`：删除上游规则（Kazumi / Venera / uzVideo）
    版权段、站点示例与 attribution；包描述改为「规则格式转换」「索引条目只提供地址」。
  - `lunac-rules/README.md`：只保留作用、使用方式、许可证；删除上游生态名称、
    attribution、生成步骤与目录说明。仓库 About 改为只说明作用。
  - 插件 UI：聚合仓库条目不再显示 upstream / attribution / 条目 id；
    已安装来源不再显示来源 id，只保留名称、版本、类型、engine 与许可。
  - 同步清理 `resource-hub/README.md` 与 `docs/README.md` 中的上游名称。
  - 验证：`typecheck` / `build` + 38 + 9 条测试全绿；浏览器导入 59 条后，
    可见文本中 `kazumi` / `venera` / `uzvideo` / `gutendex` 均为 0 处。
- **多源聚合与换源（2026-10-18）**：
  - **来源类别**：漫画 / 小说 / 视频（`anime` + `tv` 合并为视频）。来源列表与
    筛选抽屉的「来源」分页都在名称右侧显示类别标签。
  - **搜索范围**：总览搜索框左侧新增文本按钮 + 自定义下拉（非原生 `<select>`），
    可选「全部来源」或指定来源；选定后只请求该来源，下拉项右侧显示类别小字。
  - **跨源合并**：同标题、同媒体类别的作品合并为一张卡片，卡片显示「N 源」角标；
    详情页新增「换源」按钮（交换箭头图标 + 文字），并显示当前来源名称。
  - **换源面板**：从底部滑入，固定高度 58%，打开时隐藏底部三个导航按钮；
    打开后按当前作品标题自动搜索所有已安装来源，结果缓存 10 分钟（手动重试可强制刷新）。
    行布局 = 左封面 / 中间名称 + 暗色最大集数标题 / 右侧来源名称；当前来源带「当前」标记；
    章节标题后台并发读取（3 路），完成后只更新对应行，不打断列表滚动。
  - **验收**：`typecheck` / `build` 绿，38 + 9 条测试通过；浏览器实测自定义下拉、
    指定源搜索、`2 源`合并、58% 面板遮住底部导航、最新章节标题、当前标记与重试均正常。
- **视频播放（B7，2026-10-18）**：
  - 新增 `apps/lunac-plugin/src/player.ts`：MP4 直连 `<video>`；HLS 走 `hls.js`
    （`Hls.isSupported()` 优先，只有它不支持时才回落原生 `canPlayType`，判据见 §6.4）；
    多线路胶囊、WebVTT
    字幕 `<track>`、致命错误显示可读文案 + 「重试」；`timeupdate` 5 秒节流回写，
    退出时强制回写。
  - 远程来源章节点击按 `work.ref.kind` 分流：`anime` / `tv` → 播放器，
    `comic` / `novel` → 阅读器；「播放」按钮按上次 `positionSec` 续播。
  - `Progress` 增加 `positionSec` / `durationSec`；播放器回写复用 `saveProgress`，
    章节信息由 `videoSession` 补全。
  - **HLS 代理**：`lunac-media` 只代理单个 http(s) 地址，而清单里的分片 / 子清单 /
    密钥都是相对 URI。自定义 `loader` 在请求前代理、清单返回后把 URI 改写为
    **绝对原始地址**（下一跳再代理），相对解析仍然正确。
  - 宿主：CSP 新增 `connect-src` 放行 `lunac-media`（hls.js 走 XHR）；
    媒体代理补 `Access-Control-Expose-Headers`（`Content-Length` /
    `Content-Range` / `Accept-Ranges`）。
  - 验收：`typecheck` / `test`（38 + 9）/ `build` 绿；浏览器 fixture 实测
    番剧搜索 → 详情 → 剧集 → 播放器（标题 / 两条线路 / 错误 + 重试）→
    切换线路 → 退出回详情并写入进度；`video.m3u8` 200、分片 404 时 UI 正常收敛。
  - **仍待接入**：Venera / uzVideo ABI 兼容层。
- **EPUB 阅读（B6 子项，2026-10-18）**：
  - 契约：`DeclarativeRule.contentFormat`（`text` / `html` / `epub`，缺省 `text`），
    执行器透传到 `TextChapter.format`；旧规则无需改动。
  - SDK 新增 `epub.ts`：`fflate` 解 ZIP → `META-INF/container.xml` → OPF →
    spine → 每个 XHTML 抽纯文本；标题优先 NCX，其次 `h1-3` / `<title>`；
    脚本 / 样式 / head 丢弃，实体与段落换行归一。不依赖 DOM，Node 与移动端可复用。
  - 插件：`format === "epub"` 时经宿主媒体代理下载字节（`connect-src` 已放行
    `lunac-media`）→ `parseEpub` → 每个 spine 项按段拆页；`html` 走 `htmlToText`。
  - 验证：SDK 测试 38 → **42 条**（EPUB spine / NCX 标题 / 坏包 / HTML 实体 /
    `contentFormat` 透传）；浏览器 fixture 实测 3 章 EPUB 展开为 6 页，
    翻页跨章、目录、退出与进度标记均正常，`demo.epub` HTTP 200。
  - **限制**：EPUB 内的图片 / 样式暂不渲染，只读正文；spine 项当前映射为页，
    目录仍用来源章节表。
- **JS 源运行时（2026-10-18）**：
  - 宿主沙箱新增 `host.request(JSON)`：与 `net.fetch` **同一能力门**（清单必须含
    `net.fetch`）与同一套私网拒绝 / 重定向复检 / 超时与体积上限；返回信封 JSON
    （`{ok,response|error}`），不把 Rust 错误直接抛给 JS。
  - 一次性运行时之间不保留状态，但补了 **Promise 任务泵**：脚本里的 `await`
    （含 `async search`）可以正常 settle；若 Promise 依赖定时器 / 异步 IO，
    直接报可读错误而不是卡死。
  - SDK 新增 `createJsSource`：脚本导出 `createSource(rsp)`，实现
    `search / chapters / pages / content / play`，返回轻量对象；SDK 负责补
    `ref / workId / index`、解析相对地址、映射 `RspError`。
  - 宿主来源存储接受 `script` 字段（`rule` 可省略）；插件 `SourceRecord`
    增加 `script`，`engine: "js"` 的已装来源直接走 JS 运行时。
  - 验证：SDK 测试 42 → **45 条**；浏览器 fixture（`?fixture=js`）实测搜索 /
    章节 / 图片页 / 翻页 / 进度回写，控制台无错误。
  - **限制**：当前 JS 源默认使用本仓库的 `createSource(rsp)` 契约；
    Venera / uzVideo 形态分别由后续兼容层接手。
- **Venera JS ABI 兼容层（2026-10-18）**：
  - 识别：`class ... extends ComicSource` 形态的脚本自动走 `createComicSource`；
    其余 JS 源仍走 `createSource(rsp)` 契约，两者共存，无需改清单。
  - 沙箱 shim（`comic-shim.ts`）：`Network.*` / `fetch` → `host.request`；
    `HtmlDocument` / `HtmlElement` 自带容错 HTML 解析与 CSS 选择器子集
    （后代 / `>`、`.class`、`#id`、`[attr]`、`:first-child` / `:last-child` / `:nth-child`）；
    `Comic` / `ComicDetails` / `ComicSource` / `Convert`（UTF-8 / Base64 / hex）/ `UI` 最小实现。
  - 分派：`search.load` → `search`、`comic.loadInfo` → `chapters`、`comic.loadEp` → `pages`；
    相对地址按 `manifest.baseUrl` 解析，章节兼容 `Map` / 数组 / 对象三种形态。
  - 验证：SDK 测试 45 → **50 条**（识别、选择器、属性、实体、`Map` 章节、图片）；
    浏览器 fixture（`?fixture=venera`）实测 `Network + HtmlDocument` 搜索 →
    详情章节 → 图片页 → 翻页，控制台无错误。
  - **限制**：`Convert.md5/sha1/sha256/sha512/hmac/AES`、账号、收藏、分类、评论、
    图片后处理未实现，调用时给出可读错误而非返回错误数据。
- **uzVideo JS ABI 兼容层（2026-10-18）**：
  - 识别：含 `searchVideo` / `getVideoPlayUrl` 等显式函数的 type 101 扩展自动走
    `createVideoSource`；`//#BaseCode1#` 嗅探模板被识别但给出可读的不支持错误。
  - 沙箱 shim：`req`（query 参数、JSON / 文本响应、`ProData` 形态）、`cheerio.load`
    （`.find` / `.map` / `.each` / `.attr` / `.text` / `.html` / `.first` / `.eq`）、
    `UZUtils` 常用静态方法、`getEnv` / `setEnv`、平台常量与 `JSONbig`；
    按上游约定剥离成对 `// ignore` 之间的本地开发 / import 代码。
  - 分派：`searchVideo` → `search`；`getVideoDetail` 的
    `vod_play_from` / `vod_play_url` → `chapters`（多线路时标题带线路前缀）；
    `getVideoPlayUrl` → `play`（`urls[]` 优先，按扩展名推断 hls / dash / mp4）。
  - 验证：SDK 测试 50 → **55 条**；浏览器 fixture（`?fixture=uzvideo`）实测
    搜索 → 详情三集（线路1 / 线路1 / 线路2）→ 播放器挂载与标题，控制台无错误。
  - **限制**：BaseCode1 嗅探模板、网盘工具（type 300）、弹幕（type 400）已从待办删除；
    `Crypto` **已接（CryptoJS 子集，2026-10-29，见 §13.5）**，`Encrypt`（JSEncrypt / RSA）未做；
    `sniffer` **已接（2026-10-28，见 §13.5「uzVideo sniffer」）**；
    `getHome` / `getClassList` / `getVideoList` 已接入总览（2026-10-19，见 §13.5 B1）。
- **阅读器书签（B6 收尾，2026-10-18）**：
  - 共享内核新增 `bookmarks.ts`：`ReaderBookmark`（章 / 页 / 标题 / 摘要 / 时间）与
    `bookmarkKey` / `findBookmark` / `toggleBookmark` / `sortBookmarks` 纯函数；
    UI 只改副本，通过 `onBookmarksChange` 回传，移动端可复用。
  - 阅读器顶部新增「书签」面板：空状态、按阅读位置排序的列表、正文摘要（48 字）、
    当前页高亮；面板内「添加 / 移除当前页书签」，点击条目跳到对应章页
    （跨章 lazy 章节走既有 `selectChapter` 按需加载）。
  - 插件：`RhState.bookmarks` 按 `workId` 持久化，空列表删键；演示与远程来源两条
    阅读器路径都接入，重启 / 退出重进后保留。
  - 验证：reader 测试 9 → **12 条**（添加 / 去重移除 / 查找 / 排序）；浏览器实测
    P1、P2 添加、跳转、退出重进保留、移除后仅剩 P2，控制台无错误。
- **发布收尾与版本对齐（2026-10-18）**：
  - 四个 workspace（根 / SDK / reader / 插件）与 `lunac-plugin.json` 统一到
    **`0.2.0`**，`package-lock.json` 同步；插件清单位与包的版本不再分叉。
  - 全仓 `npm test`（SDK 55 + reader 12）、`typecheck`、`build` 全绿；
    产物 `dist/index.js` 1,076.03 kB（gzip 285.36 kB）。
  - README 与 §13.3 清理过期「尚未接入」口径；`dist/index.js` 与清单已同步到
    `app/src-tauri/target/debug/Modules/resource-hub/`。
- **上游条目录入与安装（2026-10-18）**：
  - 取消「只登记地址、不执行」限制：聚合仓库中带 `sourceUrl` 的条目与自建条目一样
    显示「安装」按钮。
  - 安装时经宿主 `net.fetch` 读取（20s / 2 MiB）：Kazumi JSON 转声明式规则；
    Venera / uzVideo 脚本存为 `engine: js`，`capabilities` 按上游类型补齐；
    `baseUrl` 优先从脚本里的站点地址推断。
  - 仓库侧仍不保存上游规则 / 脚本正文（分发边界不变）；安装后的正文只写入本机
    `ModuleData\resource-hub\sources`，与 AI 安装共用同一目录。
- **仓库批量安装 / 移除（2026-10-19）**：
  - 聚合仓库标题行新增「一键安装」「一键移除」：前者串行安装所有「带规则或
    `sourceUrl` 且尚未安装」的条目，单个条目失败不阻断其余，结束提示成功 / 失败数
    （失败最多列 5 个）；安装中按钮禁用并显示 `正在安装 n/N`。
  - 一键移除只清理「仓库里已安装」的来源，用户自行导入的其它来源保留；执行前
    走 `confirm` 确认，移除后同步宿主 `ModuleData` 并清理来源筛选缓存。
  - 已安装条目在仓库列表内直接提供「移除」按钮；空列表时两个批量按钮分别禁用。
  - 验证：`typecheck` / `npm test`（SDK 55 + reader 12）/ `build` 全绿；
    浏览器 harness 以两条内嵌来源实测：初始「一键移除」禁用 → 一键安装后两条
    均「已安装」→ 确认移除后恢复「安装」且已导入列表清空，console 无报错。
- **来源路由与 Kazumi XPath 修复（2026-10-19）**：
  - 根因一：安装 JS 源时把脚本自身的下载地址（Venera 的 `url` 字段，常见
    jsDelivr / raw GitHub）当成 `baseUrl`，相对封面 / 章节地址被拼到脚本仓库上。
    现优先从 `baseUrl` / `get baseUrl()` / `BASE_URL` / `webSite` 等站点字段推断，
    `url` 仅在指向站点根时兜底；推断不出时留空，不再回退到脚本地址的 origin。
  - 已安装的 JS 源在启动 / 宿主目录加载时自动重算 `baseUrl` 并写回，无需重装。
  - 根因二：Kazumi 规则的字段 XPath 是「相对当前条目」且常用 `text()` 文本节点；
    旧实现只收元素节点、字段按整篇文档求值，导致标题为空 / 所有条目取到同一链接。
    现 `selectNodes` 支持文本 / 属性节点，元素上下文下的 `//x` 自动转为 `.//x`。
  - 根因三：Kazumi `chapterRoads → chapterResult` 是「线路 → 剧集」两层结构；
    旧实现把每条线路当一集。现 `ChaptersRule.sub` 展开为
    `线路1 · 第1集 / 线路1 · 第2集 / 线路2 · 第1集`。
  - `upstream` / `attribution` 只是来源与许可元数据，不参与网络请求路由，
    继续保留；直接对接站点仍需要脚本来描述搜索与解析规则。
  - 验证：SDK 55 条测试全绿；浏览器实测 `//div[2]/text()` 抽到条目标题、
    `href` 正确、两层章节展开符合预期；typecheck / build / 产物同步完成。
  - 已知剩余：部分上游站点已下线或 403；Kazumi 播放需 WebView 嗅探，
    当前只能搜索与看目录。
- **Venera 加密 API 与面板遮挡（2026-10-19）**：
  - 兼容层补齐 `Convert`：`md5` / `sha256` / `sha512` / `hmac` / `hmacString`
    （默认 HMAC-SHA256，返回小写十六进制）与 `decryptAesCbc` / `decryptAesEcb`
    （AES-128/192/256，保留 PKCS#7 填充，与上游脚本自行去填充的行为一致）。
  - 实现为沙箱内纯 JS（不依赖 Node / WebCrypto），S 盒按 GF(2^8) 现算；
    `sha512` 使用 BigInt，运行时缺失时给出可读错误。
  - 新增 3 条对照 Node `crypto` 的测试（md5 / sha256 / sha512、HMAC、
    AES-CBC/ECB 三种密钥长度），SDK 测试 55 → **58 条**。
  - 修复隐藏的换源面板在页面滚动时露出的问题：`.rh-app` 增加 `overflow:hidden`，
    关闭状态的底部面板不再溢出到可视区。
  - 剩余：`Convert.encodeGbk`（来漫画搜索需要 GBK 表单）、`sha1`、AES 加密 /
    CFB / OFB；uzVideo 的 `Crypto` 仍是独立 shim，未接这套实现。
- **系统代理连通性与搜索实测（2026-10-19）**：
  - 经 `hongmo` 系统代理（`127.0.0.1:7892`）扫描 58 个来源站点：首页可达 47，
    HTTP 403/404 8，超时 1，无法推断站点 2；带浏览器 UA 后部分 403 变为 200。
  - 41 个 Venera 源按 `search` 实测：**21 个有结果**，9 个 0 条，11 个失败
    （含重试抖动）。拷贝漫画 30 条、Komiic 48 条、4399 24 条、爱看漫 21 条、
    图库 21 条、漫蛙吧 20 条、再漫画 20 条等。
  - 修复 `__lunac_request` 对 `ArrayBuffer` / 类型化数组 body 的处理：此前被当成
    普通对象拼成空表单，导致 `Network.post(url, headers, Convert.encodeUtf8(...))`
    全部丢 body；如漫画搜索由 0 条恢复为 9 条，并补了对应单测（SDK 58 → 59 条）。
  - 补 `randomInt(min, max)` 全局函数（少年ジャンプ＋ 的脚本依赖）。
  - 网页检索到的备用域名：包子漫画 `cn.kukuc.co` / `cn.twmanga.com` /
    `cn.webmota.com` / `cn.baozimh.org` 可访问，但脚本默认 `bzmgcn.com` 会 403；
    如漫画实际仍用 `rumanhua2.com`（接口可用，问题在 body）；
    `mycomic.xin` 是 App 官网，不是同结构的网页源。
  - 仍失败的类型：Cloudflare / 反爬（包子、爱看漫、LMM、MYCOMIC、少年ジャンプ）、
    需要登录（NoyManga）、接口变更（CCC、comick、MangaDex）、GBK 编码（来漫画）、
    关键词本身无结果（百合会、咚漫、LINE WEBTOON、MOJOIN 等）。
- **按来源覆盖域名与 GBK 编码（2026-10-19）**：
  - 清单新增 `settings`（字符串键值），安装 / 编辑时与脚本自带的 `settings`
    合并；沙箱内 `ComicSource.loadSetting(key)` 优先读覆盖值，空串视为未覆盖，
    因此旧来源行为不变。
  - 来源列表对 JS 源提供「域名」按钮：从脚本 `settings = { ... }` 中解析键名 /
    标题含 domain / host / base / api / url / site（或域名 / 地址 / 接口 / 站点）
    的字段，底部面板逐项编辑；保存前走 `validateSourceManifest`，写回
    localStorage 与宿主目录。包子漫画可把 `domains` 覆盖为 `kukuc.co`
    （脚本拼成 `https://cn.kukuc.co`，实测搜索页 200）。
  - 沙箱补 GBK：随包内置 6763 个 GB2312 汉字反查表，`Convert.encodeGbk`
    返回原始字节；请求体新增 `bodyBase64` 通路（宿主 base64 解码后按字节发送），
    GBK 表单不再被 UTF-8 化。
  - 宿主响应若是非 UTF-8（如 GBK 页面），额外回传 `bodyBase64`；
    `Network.fetchBytes` 据此返回 `ArrayBuffer`，上游的 GB2312 解码器得以生效
    （此前 lossy 成 U+FFFD，来漫画永远解析不到条目）。
  - 验证：新增字节 body / 覆盖设置 / GBK 编码 / 二进制响应 4 条测试，
    SDK 59 → **62 条**全绿；`cargo check` 通过；typecheck / build / 产物同步完成。
  - 来漫画实测：首页 GBK、表单 `/me/search/` 返回 302；结果页仍被 Cloudflare
    403（带 Cookie / iPhone UA 同样），属站点反爬而非编码问题。
- **搜索建议与来源总览（2026-10-19）**：
  - 搜索框新增建议下拉：输入时先本地即时模糊匹配（演示作品 + 已缓存远程作品），
    再以 250ms 防抖向来源补全；请求未完成时下拉内显示 `正在搜索建议…` 与
    spinner，建议项带封面、标题、来源 / 作者。
  - 输入阶段只更新下拉框，不再触发正式搜索；只有回车或搜索按钮才收起下拉框并
    进入正式数据加载（结果区显示 `正在搜索来源…` + spinner）。建议已就绪时复用
    结果，但仍保留至少 150ms 加载反馈。
  - SDK 补齐 `explore`：Venera 的 `explore[]`（`singlePageWithMultiPart` 的
    分区对象与 `multiPageComicList` 的 `{comics,maxPage}`）、JS 源
    `createSource().explore`、声明式规则的 `rule.explore`（`@section` / `@page`）。
  - 插件总览并行拉取所有声明 `explore` 来源的首页数据，与演示 / 搜索共用筛选、
    排序与合并卡片；同一来源清单 5 分钟内复用缓存，导入、移除、改域名设置、
    切回总览时按需刷新。旧 Venera 清单没有该能力时按脚本 `explore = [` 自动补上。
  - 验证：SDK 测试 62 → **64 条**；typecheck / build 绿；浏览器实测建议下拉、
    输入期间总览不变、回车后的加载态（149.9ms）、清空恢复总览，以及导入声明式
    来源后总览自动出现其 explore 卡片（`Harness 星海`）。
- **阅读器 A1–A5（2026-10-19）**：
  - **A1 左右分页整页适配**：`paged` 模式页面容器改为 flex 居中，图片
    `object-fit: contain` 限制在可视区内，不再按原始尺寸横向溢出；宽高比 ≥1.2
    的跨页大图加 `is-spread`，先显示左半页、下一次翻页显示右半页，页码标注
    「左半页 / 右半页」。
  - **A2 条漫连续拼接**：`webtoon` 模式把整章页拼成单一滚动容器
    （`.rh-reader-webtoon-item`），图片就绪后滚到目标页；滚动以视口 35% 处为
    标记回写页码，跨章自动衔接。
  - **A3 裁剪模式**：`crop` 按钮按 `off → margin → trim → split` 循环。
    `margin` 只加页边距；`trim` 用 420px 缩略图做像素扫描（亮度 ≥242 视为白）
    求内容框，再按原图比例重绘到 canvas；`split` 对宽高比 ≥1.2 的图按左右半页
    拆开。检测结果按 URL 缓存，跨域污染时回退原图。
  - **A4 手势**：Pointer / Touch 双通道；双指缩放 1–4× 并以双指中心平移，
    双击 240ms 判定放大 / 还原；下拉超过 90px（水平 <80px、未缩放、已在顶部）
    退出到详情。左右模式保留点击翻页，条漫保留滚动。
  - **A5 EPUB 图片与样式**：解包时同步收集 `jpeg/png/gif/webp/svg`，按 OPF
    相对路径解析；章节 HTML 经白名单清洗后保留 `h1-6/p/strong/em/img` 等语义
    标签与排版，图片内联为 `data:` URL（单张 ≤4 MiB），脚本 / 事件属性 /
    `href` / `style` 移除；纯图片章节不再因正文过短被丢弃。
  - **验证**：SDK 64 → **66 条**、reader 12 → **13 条**测试全绿；
    typecheck / build 绿（`dist/index.js` 1,161.13 kB / gzip 322.18 kB）。
    浏览器实测：A1 图片 295×406 落在 735×562 舞台内、跨页图占 86.94% 宽；
    A2 三个章节块 `offsetTop` 64 / 1069 / 2074，滚到 60% 页码 2/3；
    A3 trim canvas 1387×910（比例 1.5242）、split 左半 694×910；
    A4 双击 `scale(2)`、双指 `scale(4)`、下拉 140px 退出；
    A5 正文含 `strong` / `em` 与 `data:image/svg+xml`（naturalWidth 600），
    无脚本与事件属性。
- **uzVideo 总览接入（B1，2026-10-19）**：
  - `VIDEO_RUNNER` 新增 `explore`：有 `getHome` 时直接取首页（兼容数组、
    `{data}` / `{list}` / `{videos}` 与分区内嵌 `videos` / `data` / `list`）；
    首页为空时退回分类列表；否则用 `getClassList` 取前 4 个分区，再逐个
    `getVideoList({type_id,page,filter})` 合并，按 id 去重；显式传分区时只拉
    该分区并支持分页。
  - `createVideoSource` 增加 `explore`，与 `search` 共用同一套 Work 映射；
    脚本没有浏览函数时给 `unsupported`，不会伪装成空列表。
  - 安装 uzVideo 来源时按脚本静态补 `explore` 能力；已安装的旧来源在启动 /
    宿主目录加载时由 `repairSourceRecord` 自动补上，无需重装。
  - 验证：SDK 测试 66 → **70 条**（`getHome` 映射、首页为空回退、分区聚合
    与分页、无浏览函数报 `unsupported`）；typecheck / build 绿（`dist/index.js`
    1,164.45 kB / gzip 323.07 kB）。浏览器 `?fixture=uzvideo` 实测总览出现
    「星海 动画」「风起 剧场版」两张卡片、来源标签为 Harness uzVideo，
    console 无错误。
- **结果分段加载（2026-10-19）**：
  - 总览 / 搜索结果网格改为每段 30 条：`resultsHtml()` 只渲染前 `resultLimit` 条，
    底部保留 `#rh-more` 哨兵与「正在加载更多…」提示。
  - `IntersectionObserver` 以 `.rh-main` 为滚动根、提前 320px 观测哨兵；进入视口即
    `resultLimit += 30` 并重绘网格，直到没有更多时哨兵消失。
  - 回车 / 搜索、切换搜索来源、应用筛选、切换排序、刷新总览与清空输入时重置回
    第一段，避免新结果接着旧窗口渲染。
  - 验证：`typecheck` / `build` 绿（`dist/index.js` 1,166.09 kB / gzip 323.67 kB）；
    浏览器 `?fixture=uzvideo`（explore 40 条 + 其它已装来源，共 46 条）初始渲染
    30 张、`#rh-more` 存在；滚到底后增至 46 张、哨兵消失；console 0 error /
    0 warning，网络请求全部 200。
- **来源更新检查（B2，2026-10-19）**：
  - 仓库行与「已导入来源」行用 `compareVersion(entry.version, source.version)`
    标出「可更新」，并显示仓库版本与当前已装版本；无更新时仍显示「已安装」。
  - 单条「更新」与「全部更新」：按 `sourceUrl` 或内嵌 `source` 重新读取规则 / 脚本，
    串行执行、单个失败不阻断其余；完成后清换源缓存并强制刷新总览。
  - `keepSourceUserData` 保留用户的 `manifest.settings`（按来源域名覆盖等）与
    首次 `importedAt`；内嵌条目也以仓库条目的 id / 名称 / 版本为准，避免装完后
    版本仍与仓库不一致、永远显示「可更新」。
  - 验证：`typecheck` / `build` 绿（`dist/index.js` 1,171.13 kB / gzip 324.64 kB）；
    浏览器实测导入 `registry-update.json`（v1.0.2，已装 v1.0.0）后两处均显示
    「可更新」，更新后版本变为 v1.0.2、标签与按钮消失、「全部更新」禁用，
    总览卡片由「分段作品」变为新脚本的「更新作品」；console 0 error、无失败请求。
- **域名自动探活（B3，2026-10-19）**：
  - `extractDomainCandidates(script)` 从脚本里的 URL 与裸域名提取候选，过滤脚本仓库
    （jsDelivr / GitHub 等）与文件扩展名；`normalizeHost` 统一成 `host[:port]`，
    `localhost` / 回环地址按 `http` 探测，其余按 `https`。
  - 来源域名面板新增「自动探活」：按当前值 → 已保存覆盖 → 默认值 → 脚本候选的顺序，
    串行 GET 根地址（4s 超时、响应截断 2 KiB、每个字段最多 12 个候选）；命中后按字段
    原格式（带 scheme / 仅 host）填入草稿，用户点「保存」才生效。关闭 / 切换面板会
    递增代次，迟到的探测结果直接丢弃。
  - 验证：`typecheck` / `build` 绿（`dist/index.js` 1,174.90 kB / gzip 325.82 kB）；
    浏览器实测把主域名改成 `bad.invalid` 后点「自动探活」，输入框回填
    `localhost:8123` 并提示「已找到可用域名」，保存后重开仍为该值；探测
    `bad.invalid` / 旧值 `kukuc.co` 的失败为预期，无脚本异常。
- **宿主能力白名单修正（2026-10-19）**：
  - dev 日志里反复出现「插件 resource-hub 声明了宿主不认识的能力
    `media.proxy` / `engine.eval`」——两个模块（`plugin_media.rs` /
    `plugin_engine.rs`）早已实现，但 `KNOWN_PERMISSIONS` 漏登记，导致每次扫描 /
    打开插件都刷两条 warn。现补上 `media.proxy`、`engine.eval`、`storage.plugin`。
  - 该 warn 只影响日志噪音，不改变运行时授权；**如实记**：宿主日志目前不记录
    插件网络请求失败，来源报错只在插件 UI 里以「来源失败」瞬时展示，排查具体来源
    需要回到 UI 或另加落盘打点。
- **来源标识去品牌化与数据迁移（2026-10-19）**：
  - SDK 文件名与导出统一为 `comic-source.ts` / `comic-shim.ts` / `manga.ts` /
    `video-source.ts`，对应 `createComicSource` / `convertMangaRule` /
    `createVideoSource`；测试与夹具同步改名（`manga.test.ts` / `video.test.ts`）。
  - registry 的 `upstream` 值改为 `comic` / `manga` / `video`，source id 前缀改为
    `comic.` / `manga.` / `video.`；上游 URL、`license` 与 `attribution` 原样保留
    （只去我方标识，不抹上游署名）。
  - 兼容迁移：`parseRegistry` 与 `store.loadState` 归一旧前缀；宿主
    `resource_hub.rs` 在 `list_in` 读取时把旧 id 文件改名并同步 `manifest.id`
    （新增 `legacy_ids_are_migrated_on_read` 单测）。
  - 宿主目录非空时作为唯一真相源：`hydrateHostSources` 只在宿主为空时做
    localStorage → 宿主的一次性迁移；外部删除的来源不会在下次启动时复活。
  - 已删除离线体检判定为失效的 13 个来源：`venera.ffppt` / `venera.manhuaren` /
    `venera.mh4399` / `venera.mycomic` / `venera.dongman_la` / `venera.manga_dex` /
    `venera.shonen_jump_plus` / `venera.comic_walker` / `venera.baozi` /
    `kazumi.dalvdm` / `kazumi.lmm` / `kazumi.mgnacg` / `kazumi.mutefun`。
  - 验证：SDK 73 条、reader 13 条全绿；`cargo test --bin lunac resource_hub`
    5 条通过；`typecheck` / `build` 绿（`dist/index.js` 1,180.59 kB / gzip 327.52 kB）。
- **反爬请求头与登录态（B4，2026-10-19）**：
  - 来源设置面板可持久化 User-Agent / Referer / Cookie 到 `manifest.headers`；
    「设置」按钮对所有已装来源显示，声明式来源也可只覆盖请求头（无域名字段时
    「自动探活」禁用）。js / comic / video 三类沙箱统一以 `defaultHeaders` 铺底，
    单次请求同名头可覆盖。
  - 漫画源 `loadData` / `saveData` / `deleteData` 接通宿主 `storage.plugin`：
    每次 eval 前读 `source.<id>.data`，eval 结束后把变更写回（`null` 表示删除），
    登录令牌 / Cookie 可跨调用保留。
  - `sourceErrorText` 对 `blocked` / 401 / 403 / 429 / 503 追加可操作提示，
    原始错误保留在 UI，便于排查。
  - 验证：浏览器 `?fixture=comic` 保存三枚请求头后刷新仍回填、localStorage 中保留；
    `?fixture=comic` / `?fixture=videoext` 均 0 console error；视频扩展总览
    分段加载 27 → 40 张。
  - **合规边界（不做，不是待办）**：Cloudflare 类 JS 挑战**不做 WebView 绕过** ——
    那等于替用户通过站点的访问控制，会触及 Cloudflare 服务条款与目标站点条款；
    遇到挑战直接按 `blocked` 如实报错，由用户自己决定是否在浏览器里访问。
  - ~~仍未做：Cookie 目前只随来源请求发送，不注入图片 / 视频代理~~
    —— **2026-10-29 已补齐**（见 §13.5「Cookie 注入媒体代理」）。
- **总览分页、警告堆叠与卡片信息（2026-10-19）**：
  - 警告改为按来源分块的扑克牌堆叠：只展开栈顶一张，右上角显示剩余数量；
    关闭按钮与 5s 自动关闭都会把该来源记入 `dismissedWarnings` 并露出下一张，
    总览与搜索结果共用同一套计时；只有一条警告时不渲染叠牌底座。
  - 总览不再一次抓全部来源：来源清单 Fisher–Yates 洗牌后按 3 个 / 批请求，
    首屏至少覆盖 6 个有结果的来源且只渲染 30 张卡片；未加载来源不抓取，
    滚到 `#rh-more` 哨兵后再从剩余来源继续，已加载来源直接复用上方缓存。
  - 排序上方新增便携筛选行「筛选：全部 / 漫画 / 小说 / 视频 / 番剧」，
    与来源 / 标签 / 状态筛选叠加；当前窗口被滤空时自动补抓下一批来源。
  - 搜索预览按 4 个来源 / 批请求，标题按全等 → 前缀 → 包含 → 作者排序，
    每个来源只留最相关一条，最多 5 条且来源不重复；防抖 300ms，
    快速键入会 abort 上一轮请求，迟到的结果按 token 丢弃。
  - 预览卡片与详情页展示简介与标签（卡片最多 3 个标签 + 两行简介）；
    打开详情时调用 `detail()` 补全简介 / 标签 / 作者并回填 `remoteWorks`，
    返回列表后同一作品卡片随之更新。漫画源 `comic.loadInfo` 与视频源
    `getVideoDetail` 分别映射到 `description` / `tags`。
  - 验证：`typecheck` / `test`（SDK 76 条 + reader 13 条）/ `build` 绿
    （`dist/index.js` 1,191.99 kB / gzip 330.68 kB）；浏览器 `?fixture=videoext`
    首屏 30 张、触底增至 43 张，第一张卡片含 3 个标签与简介，筛选行位于排序
    chips 上方，点「漫画」后只剩 1 张、点「全部」回到 30 张首段；
    `?fixture=broken` 警告 5s 自动消失、刷新后手动关闭立即消失；
    快速键入「星海」预览 2 条且来源不重复（Harness VideoExt / 演示）；
    console 仅有一条预期内的旧搜索请求 `ERR_ABORTED`。
  - 局限：单个夹具只能验证一条警告，叠牌的多来源外观按代码路径验证；
    harness 以 `?fixture=broken` 复现失败来源。
- **总览预览缓存与安装预热（2026-10-19）**：
  - 总览结果按来源写入宿主 KV：键 `overview.v1.<sourceId>`，值
    `{ id, name, sig, at, works }`；`sig` = 来源版本 + `manifest.settings`，
    版本或设置变化即视为过期；每个来源最多缓存 24 条作品，简介截断 200 字、
    标签最多 6 个，避免单个值超过宿主 256 KiB 上限。
  - 启动时先读 KV 缓存并立即渲染，不等网络；缓存覆盖全部已装来源且
    30 分钟内时完全不联网。缓存缺失 / 过期时只补缺失来源，且在后台逐批刷新，
    已显示的卡片不会被清空；新结果按 `sourceId` 原位替换，不会与缓存重复。
  - 安装 / 更新来源后立刻调用一次 `explore` 抓预览并写缓存
    （`prefetchSourcePreview`），装完即可在总览看到该来源；批量安装串行预热，
    手动导入用 `previewPrefetching` 保护、避免与总览刷新重复请求。
    卸载来源时同步删除它的缓存键。
  - 落点：`<exe 根>\ModuleData\plugin-data\resource-hub\kv.json`（宿主
    `plugin_kv`，按插件 id 隔离、原子写）。清单新增 `storage.plugin` 权限；
    dev 插件目录必须叫 `Modules\resource-hub`（与 manifest.id 一致），
    否则 `plugin_kv` 的能力门读不到清单会回 `E_DENIED`。
  - 验证：`typecheck` / `build` 绿（`dist/index.js` 1,197.68 kB / gzip 332.34 kB）；
    浏览器 `?fixture=comic` 首屏 5 张、`comic-search.html` 只请求 1 次；
    「来源 → 总览」切换 0 次新增抓取、卡片保持；刷新后重新抓取 1 次
    （harness 的宿主存储是内存态，真实宿主持久化到 kv.json）；console 0 error。
  - dev 部署修正：`Modules\dist` 曾与 `Modules\resource-hub` 同 id 重复，
    已把旧副本改名为 `.dist-backup-20261019`（宿主扫描跳过 `.` 开头目录），
    只保留 `Modules\resource-hub` 的最新产物。
- **统一加载条、来源专属视图与卡顿治理（2026-10-19）**：
  - 原来顶部「正在加载来源数据…」与底部「正在加载更多…」合并为一个加载区：
    首次无结果时是整块 `#rh-source-loading`，有结果后是底部 `#rh-more`；
    文案统一为「正在加载来源 3/46 · 来源名」/「正在搜索来源 2/2 · 来源名」，
    抓取完成后空闲态显示「继续下滑加载更多」。`runtime.ts` 的
    `searchSources` / `exploreSources` 新增 `onSourceDone` 回调，逐个来源上报进度。
  - 左上角来源下拉选中具体来源后进入该来源的专属内容页：总览只展示该来源、
    内置示例不参与，未缓存时先显示整块加载界面并只抓这一个来源；
    30 分钟内且有缓存时直接展示不联网；滚到底仍可继续翻该来源的条数。
    选回「全部来源」恢复跨源混排。
  - 卡顿根因与修复：
    ① `IntersectionObserver` 每次重绘都会重新 observe，哨兵仍在视口时立即回调，
       形成「抓一批 → 整网格重绘 → 立即再抓」的循环；现在只有真实滚动才「上膛」，
       并加 350ms 冷却与哨兵可见性检查（`maybeAutoMore`）。
    ② 打开来源下拉会 `paint()` 重建整个应用与网格（图片/DOM 全部重建、滚动归零）；
       现在只切换菜单 class / aria，网格与滚动位置保持（实测 scrollTop 400 → 400）。
    ③ 警告卡每次 5s 自动关闭都整网格重绘；现在只更新 `#rh-warnings`。
    ④ `resultItems()` 在每次 render / paint 都重新排序、去重所有来源；
       现在按「查询 + 来源 + 筛选 + 排序 + 数据版本」缓存，数据变化时递增版本失效。
    ⑤ 加载进度逐个来源完成时只更新加载节点文本，不重绘网格。
  - 验证：`typecheck` / 76 + 13 条测试 / `build` 绿（`dist/index.js`
    1,204.93 kB / gzip 334.23 kB）；浏览器 `?fixture=videoext` 首屏 30 张，
    下拉展开 scrollTop 保持 400，选「Harness VideoExt」后 40 张全部为
    「分段作品」且无内置示例，切回「全部来源」示例重新出现；
    搜索「星海」时加载条显示「正在搜索来源 2/2 · Harness VideoExt」；
    旧文案「正在加载更多…」/「正在加载来源数据…」全页 0 次；console 仅有
    历史页面导航错误与一条预期内的演示来源 `ERR_ABORTED`。
- **预览卡片与详情页解耦（2026-10-19）**：
  - 调查结论：总览 / 搜索预览只消费 `explore` / `search` 返回的 Work，
    **不会调用 `detail()`**。浏览器 `?fixture=comic` 取证：总览阶段
    `comic-info.html` 请求 0 次，只有搜索页与封面；点击远程漫画卡片后
    才出现 1 次 `comic-info.html`（详情页 + 章节共用同一 GET，30s 网络缓存去重）。
  - 卡片不再展示简介：`gridCard` 移除 `.rh-grid-desc`（保留标题、来源、
    状态与最多 3 个标签），减少 DOM 高度与渲染量；简介与标签仍完整保留在详情页。
  - 详情补全从 `loadRemoteChapters` 移到详情页专用 `ensureRemoteDetail`：
    阅读器 / 播放器 / 继续阅读路径不再顺手拉详情；同一作品只尝试一次，
    避免详情页反复绘制时重复请求。
  - 说明：`comic.loadInfo` / `getVideoDetail` 在「详情 + 章节」时会被调用两次
    （SDK 层的第二份结果走 30s 网络缓存，不重复触网，但 QuickJS 仍会执行两次）；
    本次只收敛到详情页触发。纯浏览器 harness 的 `engineRequest` 使用同步 XHR，
    会阻塞页面主线程，这是 harness 环境的固有现象；dev 宿主走异步 Tauri 命令。
  - 验证：`typecheck` / `build` 绿（`dist/index.js` 1,205.40 kB / gzip 334.38 kB）；
    浏览器复测：总览 5 张卡片、`.rh-grid-desc` 0 个、`comic-info.html` 0 次；
    点击远程卡片后新增 1 次详情请求，详情页简介与标签正常；console 0 error。
- **全部来源单源预览、去分页与搜索栏选源（2026-10-19）**：
  > 2026-10-21 已推翻「去分页」这一条：搜索与总览都改成按来源 `Paged.next`
  > 真分页（见下方「搜索 / 总览真分页」）。单源随机预览仍保留。
  - 移除「全部来源前 30 张必须来自不同源」的凑数逻辑：`OVERVIEW_SOURCE_BATCH`
    由 3 改为 1，总览只随机挑 **一个** 可预览来源抓取（`explore` 一次），
    不再为了填满首屏而强制加载多个来源；首屏卡片允许全部来自同一来源。
  - 「全部来源」不再分页：`resultsHtml()` 的 `allOverview` 分支一次性渲染全部
    已有结果，不生成 `#rh-more` 与触底续拉；选中具体来源时才保留 30 条一页的
    纯前端窗口（`loadMoreResults()` 只扩大 `resultLimit`，不联网）。
    `fillResultsWindow()` 与多批次 `loadNextOverviewBatch()` 循环一并删除。
  - 搜索栏右侧新增原生来源选择器 `select#rh-search-source`（全部来源 + 已安装来源），
    替换原自定义弹出菜单（删除 `searchSourceOpen` / `toggle-search-source` /
    `pick-search-source` 与 `.rh-search-menu`）；`change` 事件统一走 `pickSource(id)`：
    有查询时重跑搜索，无查询时进入该来源专属预览或回到全部来源随机预览。
  - `resultItems()` 与 `activeWarnings()` 的 `onlySource` 统一取 `searchSource`，
    指定来源的 **搜索** 也只展示该来源（内置演示示例不参与），与总览行为一致。
  - `runtime.ts` 新增 `SOURCE_CONCURRENCY = 4` + `mapLimited()`：搜索 / 总览的
    来源脚本最多 4 个并发，避免几十个 QuickJS 沙箱同时执行拖死主线程；
    `logSourceRun` 在单源耗时 ≥1s 时输出 `warn`。
  - 验证：`npm test` 76 + 13 条全过、`typecheck` 绿、
    `dist/index.js` 1,209.30 kB / gzip 336.18 kB。
  - 浏览器 `?fixture=comic`：首屏仅 1 次 `batch start {sources: 百合会, queue: 1}`、
    `overview ready {previewSources: 1}`、3 张卡片同源、`.rh-overview-hint` 存在、
    `#rh-more` 不存在；`select#rh-search-source` 共 16 个选项（x=520，位于搜索按钮
    x=476 右侧）；全来源搜索「星海」`search start {sources: 15}`。
  - 浏览器 `?fixture=videoext`：选「Harness VideoExt」首屏 30 张全部为该来源，
    触底 40 张且 console 无新增 `batch start`；该来源搜索「星海」2 张全部同源、
    无内置示例，`search start {sources: 1}`。
  - 遗留：harness 的 `engineRequest` 为同步 XHR，重复冷加载仍偶发
    `main thread blocked 1777ms` / `longtask 137ms`；常驻来源将按需降权/跳过。
- **渲染层 P0 优化（2026-10-19）**：
  - `.rh-grid-card` 增加 `content-visibility:auto` + `contain-intrinsic-size:auto 248px`：
    屏外卡片的布局 / 绘制由浏览器跳过，滚动时按估算高度占位；选中 / 键盘聚焦时自动恢复渲染。
  - 卡片封面改为 `<img data-src … loading="lazy" decoding="async">`，
    `observeCovers()` 用 `IntersectionObserver`（root = `.rh-main`，`rootMargin:300px`）
    在接近视口时才写入 `src`；原生 lazy 的 1250 / 2500px 预取距离不再一次性拉取整屏外原图。
  - 「全部来源」渐进渲染：首段 `OVERVIEW_RENDER_STEP = 60` 张，其余由
    `requestIdleCallback`（`timeout:200`，无该 API 时 `setTimeout 24ms`）按每批 60 张
    `insertAdjacentHTML` 追加；仍**无分页、无加载更多、不新增联网批次**。
    `cancelProgressiveRender()` 用代次令牌在换数据 / 重绘时作废旧链。
  - `paint()` 合并到 `requestAnimationFrame`：同一帧内多次状态变化只重建一次整树；
    另加 120ms `setTimeout` 兜底，避免窗口隐藏 / 后台标签页暂停 rAF 时界面停在旧状态。
    `paintNow()` 保留原整树重建逻辑，并在每次重绘后重新挂载封面观察器。
  - 验证：`typecheck` 绿、`npm test` 76 + 13 全过、
    `dist/index.js` 1,212.96 kB / gzip 337.31 kB。
    浏览器 `?fixture=videoext`：`contentVisibility` 实测 `auto`、首张封面含
    `decoding="async"`、全来源首屏无 `#rh-more` 且 `.rh-overview-hint` 存在；
    选「Harness VideoExt」30 张，滚底 40 张且 `#rh-more` 消失，console 无新增
    `batch start`；切「书架」再切回「总览」正常重绘；无 `paint` / `longtask` /
    `main thread blocked` 告警。
- **长期后台打开：加载数据时不进加载页（2026-10-31 报障）**：
  - **现象**：插件窗长期留在后台（最小化 / 被遮挡），之后触发加载时界面没有加载页，
    切回来时内容一次性出现、还顿一下。
  - **根因一（主因，代码顺序）**：`refreshOverview()` 先 `await hydrateOverviewCache()`，
    之后才置 `exploreLoading = true`。而读缓存 = `storage.keys()` + **每个来源一次**
    `storage.get()`（40+ 个来源就是 40+ 次宿主 IPC），这段时间界面**没有任何反馈**。
    长会话 / 来源变多 / 磁盘慢时这段空窗被放大成「点了没反应」。
  - **根因二（渲染时机）**：`paint()` 靠 `requestAnimationFrame`（+120ms 兜底）。窗口被遮挡
    或长期在后台时 rAF 被挂起、后台定时器被节流（最狠 1 次/分钟）⇒ 「开始加载」这个状态
    可能一直画不出来，直到用户切回来才一次性补绘（体感 = 卡）。
  - **修法**：① 先把「加载中」画出来再读缓存（`force` 或当前没有结果时）；
    ② 新增 `paintImmediate()` —— 取消挂起的 rAF/定时器、按当前状态同步重画；
    `document.visibilityState === "hidden"` 时 `paint()` 直接走它；回到可见时
    （`visibilitychange`）若有挂起重绘也立刻 flush；③ 缓存读取**限并发 8**，
    不再把几十个 IPC 同时塞进宿主队列。
  - **验证**：`typecheck` / `build` exit 0，SDK 98 + reader 13 测试全过；
    插件版本 **0.2.5**、产物 **1,286,448 B**，已覆盖 dev `Modules\resource-hub`；
    验收口径见 `docs/testing.md` §测试 21.14。
  - **仍未做**：没有在真机上量到「rAF 从挂起到恢复」的实际毫秒数；若仍复现，
    下一步是在 `paint()` 记「请求重绘 → 真正绘制」的间隔（临时日志），而不是继续猜。
- **AppHang 根因与媒体代理异步化（2026-10-20）**：
  - 取证：Windows 事件日志为 `Application Hang / AppHangB1`（不是崩溃转储），
    即宿主 UI 线程连续 >5s 不处理消息，被系统判定「未响应」（历史记录
    10-19 19:33 / 19:37 / 21:50 已出现，P0 前就存在）。
  - 根因：`main.rs` 用 `register_uri_scheme_protocol` 注册 `lunac-media`，
    同步协议回调会在 WebView UI 线程上执行；`plugin_media::handle` 里是
    `reqwest::blocking` + DNS 私网校验 + TLS 握手，一屏几十张封面就会把消息泵堵死。
  - 修复：改用 `register_asynchronous_uri_scheme_protocol`，
    `plugin_media::dispatch` 只入队；新增 6 个 `lunac-media` 工作线程的有界池，
    网络读取与 32 MiB body 读取全部移出 UI 线程。协议池关闭时兜底同步执行。
  - 同时落地 P1：媒体代理复用全局 `reqwest::blocking::Client`（连接池 / keep-alive），
    图片响应 `Cache-Control: private, max-age=86400`，非图片仍 300s。
  - P1 数据侧：失败的来源进入 5 分钟冷却（`SOURCE_COOLDOWN_MS`），
    「全部来源」随机预览跳过冷却中的源（手动选源仍可立即重试）；
    `remoteWorks` 加 2000 条 LRU（`rememberRemoteWork`），长会话不再无限增长。
  - P2 取舍：真实虚拟滚动暂缓（`content-visibility` 已覆盖主要收益）；
    面板隐藏暂停与独立 WebView 进程隔离需要宿主级改造，留作后续。
  - 验证：`cargo check` / `cargo build` 绿（`lunac.exe` 12:50 重新产出）；
    `typecheck` 绿、`npm test` 76 + 13 全过、
    `dist/index.js` 1,213.76 kB / gzip 337.62 kB；
    浏览器回归：选「Harness VideoExt」30 张、进详情「分段作品 1」、返回仍 30 张，
    无未捕获 JS 错误，仅 1 条 175ms 首屏 longtask（harness 同步 XHR）。
- **媒体代理进程隔离（2026-10-20）**：
  - 定位：Defender 的 `Behavior:Win32/LummaStealer.CER!MTB` 只会在阅读插件
    启用 `media.proxy` 时命中；逐项删权限实测证明 `engine.eval` / `net.fetch` /
    伪造 Referer/UA 都不是触发点，宿主亲自取任意 URL 才是（详见
    `docs/testing.md` §测试 12.1）。
  - 实现：新增独立 crate `app/media-proxy`（`lunac-media-proxy.exe`），
    监听 `127.0.0.1:<随机端口>`，只接受带 token 的 `GET|HEAD /fetch`，
    自己完成远程取回（含 Range、私网拒绝、重定向逐跳复检、32 MiB 上限）。
  - 宿主：`plugin_media::handle` 仍负责能力门与目标 URL 校验，但不再直接访问远程；
    改为 `media_proxy::fetch` 转发到 helper。helper 由 `child_job` 绑定，随宿主退出；
    掉线时重启并重试一次；端口 / token 每次启动随机生成，只暴露给宿主。
  - 构建：`app/package.json` 的 `build` / `dev:media-proxy` 先编 helper release；
    `tauri.conf.json` 的 `bundle.resources` 把 `lunac-media-proxy.exe` 放到安装目录；
    `tauri:dev` 会先编 release helper（`tauri-build` 会校验资源存在）。
  - 验证：不在 Defender 排除名单的 `lunac-test.exe` + CDP 打开阅读插件，
    25s 后宿主存活、日志出现 `media proxy helper started`、宿主与 helper
    均无新增 `Get-MpThreatDetection` 记录；helper 在宿主退出后由 Job Object 收掉。
- **dev 副本残留清理与章节诊断（2026-10-21）**：
  - 现象：插件侧先报 `E_DENIED 未声明 net.fetch / engine.eval`（来源抓取全失败），
    权限补回后又出现「资源预览图 / 章节内容全空」。
  - 根因是两处 dev 副本残留，都不是源码问题：
    ① `app\src-tauri\target\debug\Modules\resource-hub\lunac-plugin.json` 还是
    Defender A/B 实验删权限后的版本（只剩 3 项）；
    ② 同目录 `index.js` 还是实验版，封面与章节图走 `http://127.0.0.1:8124/fetch?url=`，
    而那个本地 Node 代理早已停掉（`naturalWidth` 全为 0）。
  - 修复：`resource-hub` 重新 `npm run typecheck` / `npm run build`，把
    `dist/index.js` + `dist/lunac-plugin.json` 覆盖回 dev 副本；清单恢复五权限，
    `index.js` 与 dist 哈希一致且不再含 `8124`。
  - 验收：封面改为 `http://lunac-media.localhost/resource-hub/<encoded-url>`
    并正常解码（首屏 15 张里 12 张出图），详情页正常；`E_DENIED` 消失。
  - 诊断增强：`loadRemoteReaderChapter` 的 `catch` 由静默改为
    `perfWarn("chapter load failed", { source, chapter, error })`，
    避免界面只剩一句「章节加载失败」而无法区分「上游反爬」与「规则失配」。
  - 仍待观察：`百合会` 单章 `loadEp` 返回 `parse`（`img#imgPic` 未命中或状态码非 200），
    需按控制台新日志判断是上游反爬还是来源规则需要更新。
    （2026-10-28 复核：**已通过**，结论见本节「百合会 loadEp 复核」。）
  - 章节加载失败的定位（同日续查）：
    - 用 Node + curl 复现 `createComicSource`：`chapters` 88 条、第一章 `pages` 19 张，
      说明来源脚本与 HTML 解析本身没问题，问题在宿主链路。
    - 宿主侧证据：异常栈落在 `__lunac_request`（`host.request` 失败），
      而非状态码检查；`plugin_net` 原先**每次请求都新建 reqwest Client**，
      19 页就要重新握手 19 次，叠加 `engine.eval` 的 25s 看门狗，
      长章节会以「不带 message 的 JS 异常」收场。
    - 修复：`plugin_net` 改为共享 `reqwest::blocking::Client`（连接复用，超时改在请求级）；
      `comic-source` / `video-source` / `js-source` 的 `EVAL_TIMEOUT_MS` 统一取宿主上限 30s。
    - 诊断增强：`host.request` 失败写宿主 warn（`plugin <id> host.request 失败：<原因>`）；
      `engine.eval` 触发看门狗时写 `plugin <id> engine.eval 超时（<ms> ms）`——
      否则插件侧只能拿到 QuickJS 的裸栈，无法区分超时与请求失败。
    - 第二层根因（日志证实，同日）：耗时修掉后又报
      `host.request 失败：请求参数不合法：invalid type: map, expected a string`。
      上游 Venera 来源存在两种写法：`Network.get(url, { "User-Agent": "…" })`（第二参就是头对象）
      与 `Network.get(url, { headers: { … }, body? })`（第二参是 options）；
      `comic-shim` 的 `__lunac_request` 把 options 对象直接当头对象，头值变成 map，
      Rust 侧 serde 直接拒掉整次请求。
      修复：识别 `headers.headers` 并解包（顺带取 `headers.body`），对象值 `JSON.stringify`、
      其余 `String()`，跳过 null / undefined。
      回归测试：`comic.test.ts` 新增「Network 的 options 形态（`{ headers, body }`）被解包成字符串头」，
      断言头值全为字符串、POST body 原样透传。
    - 重建验证（2026-10-21）：`typecheck` 绿、SDK 78 + reader 13 全过、
      `dist/index.js` 1,215.98 kB / gzip 338.41 kB；已覆盖 dev 副本（与 dist 哈希一致，
      含 `headers.body` / `headers.headers` 解包与 `sniff` 标记）。
      重启 dev 实例后点第一章，日志不应再出现 `invalid type: map, expected a string`。
  - 番剧 / 视频抓不到的结论：
    - 已装番剧源（`manga.*`）全部是 Kazumi 声明式规则，`capabilities` 只有
      `search,chapters`，**没有 `explore`，也没有 `play`**；`convertMangaRule` 只产出这两项。
    - 因此：总览（只跑 `explore`）永远不包含番剧源；点开剧集必然报
      「没有 play 规则」。`useWebview` 嗅探在 v1 明确不执行（转换器会写 warning）。
    - 可选路线：① 宿主实现 WebView 嗅探（对齐 Kazumi 的 useWebview，工作量在 Rust 侧）；
      ② 导入 uzVideo type 101 的 JS 源（`getVideoPlayUrl` 已由兼容层接入）。
  - **已落地路线 ①：WebView 嗅探（2026-10-21）**：
    - 宿主新增 `src/plugin_sniff.rs`：`plugin_sniff_media(pluginId, url, timeoutMs)` 开一个
      **不可见、不进任务栏**的 WebView2 打开剧集页，拦截
      `.m3u8` / `.mp4` / `.flv` / `.mkv`（最多 8 条），拿到首条或超时（缺省 15s，2–30s）
      后立即关窗；只回传地址，不读页面内容、不注入脚本。
    - 能力门：清单新增 `permissions: ["webview.sniff"]`，未声明回 `E_DENIED`；
      目标地址复用 `plugin_net::validate_url`（只允许 http(s) 公网）。
    - 桥：`__lunac_host.sniff.media(url, { timeoutMs? })`——作为 **apiVersion 3 的增补字段**
      （不递增版本，避免旧插件被 `requireHostV3` 打成 mock）。
    - 声明式规则新增 `sniff: { url, timeoutMs? }`（与 `play` 二选一）：
      `executor.play()` 先按 `@chapterUrl` 开嗅探，再把地址映射成 `hls` / `mp4` 线路。
    - Kazumi 转换器：有章节的番剧规则自动产出 `capabilities: ["search","chapters","play"]`
      与 `sniff: { url: "@chapterUrl" }`；`runtime.sourceFor` 对**已装旧规则**在读取时补同一条
      sniff，用户不需要重装来源。
    - 验证：SDK 77 条 + reader 13 条测试通过，`cargo build --bins` 通过；
      `webview.sniff` 已写进 `agent-templates/modules/README.md` 的能力表。
    - **修正（2026-10-21，番剧仍抓不到）**：首版用 Tauri 的 `on_web_resource_request`
      挂在 `WebviewWindowBuilder` 上，真机上永远「没嗅探到」。查 tauri 2.11.5 源码：
      该回调只在 `tauri://` 自定义协议响应里被调用（`protocol/tauri.rs`），
      wry 也只对 `custom_protocols` 注册 `AddWebResourceRequestedFilter`——
      `WebviewUrl::External` 打开的远程页面收不到任何请求。
    - 现实现：窗口先开 `about:blank`，经 `with_webview` 拿到 `ICoreWebView2`，
      调用 `AddWebResourceRequestedFilter("*", CONTEXT_ALL)` + `add_WebResourceRequested`
      **先挂拦截再 `Navigate`**（避免首批媒体请求漏掉）。
      依赖 `webview2-com 0.38` / `windows-core 0.61`（都随 wry 在 `Cargo.lock` 里，不新增下载）；
      非 Windows 目标编译为返回 unsupported 的桩。
    - 可观测性：宿主日志新增 `plugin <id> webview sniff start <url>` /
      `webview sniff 命中 <url>` / `webview sniff 完成：N 条地址` /
      `webview sniff 超时（<ms> ms）没有媒体地址`，并给 `plugin_market::KNOWN_PERMISSIONS`
      补上 `webview.sniff`（此前每次加载都会 warn「宿主不认识的能力」）。
    - 回归：`cargo test --bins` 306 passed / 2 ignored；SDK 78 + reader 13 全过；
      `dist/index.js` 1,216.68 kB / gzip 338.63 kB，已同步 dev 副本。
    - 仍待真机验证：番剧搜索 → 详情 → 剧集 → 播放器。
      （总览 / 分类不含番剧源的问题已在下一节用「空关键词浏览」修掉。）
- **分类视频空白 / 标签按源分组 / 筛选抽屉（2026-10-21）**：
  - 现象：把分类切到「视频 / 番剧」时一张卡都没有；更多筛选里的标签不区分来源；
    搜索栏右侧的来源下拉与筛选功能重复。
  - 根因：总览只跑 `capabilities: ["explore"]` 的来源，而已装番剧源全是 Kazumi
    声明式规则（只有 `search` + `chapters`），因此永远不进总览。
  - 修复（`runtime.canBrowse`）：HTML 搜索规则的番剧源在读取时补一个 `explore`，
    用**空关键词搜索**当「全部 / 近期更新」入口（MacCMS 等站点会返回列表）；
    JSON API 规则不做此兜底（空查询必然失败）。总览与分类切换后立即按
    `filterKinds` / `filterSources` 重新拉取对应来源。
  - 连接级请求头：`plugin_net` 原先把 `Connection: keep-alive` 当非法头**整次拒绝**，
    而 Venera / uzVideo 源普遍带它 ⇒ 部分来源全部请求失败。改为校验时放行、
    发送时丢弃（`host` / `content-length` / `connection` / `transfer-encoding`），
    并补回归测试。
  - 标签：声明式 HTML 列表在规则没给 `tags` 时，从卡片节点里的
    `a[href]`（`/tag`、`/type`、`/vodtype`…）与 `class*=tag` 收集，最多 4 个；
    更多筛选的「标签」页改为**按来源分组**：一个源只列自己的标签，选中的源即使
    没抓到标签也会显示「该源暂无可筛选的标签」。
  - 筛选 UI：删除搜索栏右侧的来源下拉（改由筛选抽屉统一控制）；
    「更多筛选」从工具栏移到**左下角悬浮按钮**，打开**左下角小抽屉**，
    抽屉头部与按钮文案都显示**当前筛选的源**（未选时为「全部来源」）。
  - 验证：SDK 78 + reader 13 全过；`cargo test --bins` 306 passed / 2 ignored；
    `dist/index.js` 1,218.14 kB / gzip 339.21 kB，已同步 dev 副本。
- **来源管理搜索 / 标签 pill / uzVideo 索引（2026-10-21）**：
  - 左下角筛选：悬浮按钮右侧新增**已选标签 pill**，每个 pill 带关闭按钮，
    点 × 立即从 `filterTags` 移除并重算结果（无需再开抽屉）。
  - 来源管理：新增搜索框（名称 / id / 域名，即时过滤）；「聚合仓库」与「已导入」
    两个分区**默认折叠**，点标题展开，标题右侧显示当前可见条数。
  - 拼音检索结论：阅读插件的搜索建议与本地匹配只做 `toLowerCase + includes`
    （标题 / 作者 / 简介 / 标签），**不支持拼音全拼或首字母**；远程来源由站点自身
    搜索处理。宿主虽有 `pinyin` crate（快速启动 / 文件索引在用），但没有给插件的桥，
    要支持需新增宿主能力（本次未做）。
    **2026-10-22 已补**：宿主 `pinyin_encode` + 插件 `matchPinyin`，见本节末尾「拼音检索」条。
  - 音乐插件本地播放：**已支持视频** `mp4 / m4v / mov`（宿主 `VIDEO_EXTS` +
    WebView 画面层，声音仍走 rodio）；`mkv / avi / webm / wmv / flv` 刻意不收
    （Chromium 不出画面，或音轨是 Opus 导致「有画面没声音」）。
  - uzVideo：新增 `registry/upstream/uzvideo.json`（从
    `YYDS678/uzVideo-extensions` 的 `vod/js/*.js` 生成 45 条，跳过 `Deprecated_`），
    并修掉 `build-registry.mjs` 只认 `config.video`、忽略 `config.uzvideo` 的问题。
    现在 `registry:build` 产出 17 manga + 41 comic + 45 video + 1 manual = 104 条。
    注意：插件默认拉取的是发布后的 `lunac-rules` 仓库，本地索引需要发布后才对用户可见。
  - 验证：`typecheck` 绿、SDK 78 + reader 13 全过、`registry:build` 通过、
    `dist/index.js` 1,222.45 kB / gzip 340.19 kB，已同步 dev 副本。
- **CMS 采集源入库 + 连通性探测（2026-10-21）**：
  - 新增声明式 `cms` 规则（`CmsRule`）：只写采集接口地址，执行器按 Apple CMS v10
    约定完成 `search`（`?ac=detail&wd=&pg=`）、`explore`（`?ac=detail&pg=`）、
    `chapters`（`?ac=detail&ids=`，拆 `vod_play_from` / `vod_play_url` 的线路与剧集）、
    `play`（直链 m3u8 / mp4 直接播放；采集站给的是播放页时回退 `webview.sniff`）。
    剧集地址直接存进 `Chapter.id`，播放不再二次请求详情。
  - `registry/upstream/cms.json` 收录本轮探测通过的 6 个接口，由 `fromCmsIndex`
    生成**内嵌规则**条目（`cms.<key>`，`upstream: manual`），导入仓库后可直接安装：
    百度 / 暴风 / 虎牙 / 量子 / 非凡 / 火狐。
  - 探测口径：先直连（这些都是国内站点）；`raw.githubusercontent.com` 在本机直连被
    `ECONNRESET`，索引文件改走 `ghproxy.net` 镜像；GitHub API 镜像返回
    `Invalid input`（不可用）。
  - 本轮跳过（无用或不可达）：`suoniapi.com`（关闭搜索，仅列表）、`lzizy1.com`
    （搜索 403）、`cj.rycjapi.com` / `zy.jinchancaiji.com`（uzVideo 里的 CMS，
    连接重置 / 域名不存在）、`haiwaikan.com`、`api.kuaifan.tv`、`caiji.dyttzyapi.com`、
    `api.wujinapi.me`（403）、`cj.ukuapi.com`、`json.heimuer.xyz`（302）。
  - MoonTV / LibreTV 结论：前者仓库只剩指路 README（已迁 `MoonTechLab/LunaTV`），
    两者都是**空壳不内置采集站**，没有可直接复制的源列表；候选来源改用公开 TVBox
    配置（`gaotianliuyun/gao`、`liu673cn/box`）里的 `api.php/provide/vod` 站点表。
  - 验证：SDK 82 条测试全过（新增 3 条执行器 + 1 条 registry）、`typecheck` 绿、
    `registry:build` 产出 17 + 41 + 45 + 6 + 1 = 110 条；
    `dist/index.js` 1,226.82 kB / gzip 341.30 kB，已同步 dev 副本（哈希一致）。
  - 发布：`lunac-rules` 已推送 `c466ce7`（index.json 59 → 110 条、62,267 bytes），
    jsDelivr 已 purge（CF / FY 均成功）。
- **手机竖屏适配（2026-10-21）**：
  - `rh-reader` 新增手机紧凑断点（`max-width:640px` 或 `max-height:480px` 且触屏）：
    底部 6 个动作按钮压成单行、只留图标（速度 / 模式保留数值），按钮 44px；
    页留白 `52/14/112/14`、正文 1.02rem；目录面板 92% 宽；
    非条漫图片上限 `100dvh - 128px`（横屏不再被平板断点的 196px 压扁）。
  - 手势：左右分页新增**单指横滑翻页**。实现放在 `touchend`：
    Chrome 在拖动开始即发 `pointercancel`，pointerup 方案实测收不到事件；
    竖向 / 条漫模式不响应横滑。
  - 细节：stage 补 `-webkit-touch-callout:none`；`matchMedia` 跨断点时只重绘上下框文案。
  - 验证：CDP 375×812 / 812×375 / 1280×800 三档实测（数值见 §8.5.1）；
    reader 13 条测试与 `typecheck` 通过；`dist/index.js` 1,229.69 kB / gzip 341.93 kB，
    已同步 dev 副本（哈希一致）。
  - 移动端仓库 `lunac-reader-mobile` 同步共享内核（此前停在 0.1.0 子集）：
    已推送 `3094591`（47 个文件，删除 `kazumi.*` 旧文件），
    该仓库 `typecheck` / `build` / SDK 82 + reader 13 测试全过。
- **HLS 播放修复 + 换源搜索收敛（2026-10-21）**：
  - **根因（播放）**：WebView2（Edg/155）对 `application/vnd.apple.mpegurl` 的
    `canPlayType` 回 `maybe`，播放器先走原生分支 → `<video>` 把代理地址当 HLS 直接播，
    永远停在 `readyState 0`（`networkState 2`），控制台无错误、UI 一直「正在加载视频…」。
    修法：`player.ts` 改为 `Hls.isSupported()` 优先，原生只在 hls.js 不支持时兜底；
    同时把 `manifest/level/frag` 超时从 10s 放宽到 30s（媒体要多过一跳代理，
    虎牙 / 暴风 CDN 实测清单 6–16s 才返回），并允许清单 2 次、分片 3 次重试。
  - **根因（换源）**：`runSourceSwitchSearch` 一把梭搜**全部 110 个已装来源**
    （含走 `engine.eval`、看门狗 30s 的 JS video 源），不按类型过滤、不传
    `AbortSignal` 与超时、且 `sourceSwitchLoading` 时直接 `return` 让「重试」失效 ⇒
    抽屉长时间「正在搜索其他来源…」。
  - 修法：只搜与当前作品 `sameMediaKind` 的来源；`AbortController` + 15s 硬上限
    （`SOURCE_SWITCH_TIMEOUT_MS`），关闭抽屉 / 重新搜索先 abort 上一轮；
    `searchSources` 在 `signal.aborted` 后不再启动新来源；结果用 `onSourceDone`
    边搜边追加（`sortSourceSwitchItems` 统一排序），抽屉里「结果 + 底部进度行」并存。
  - 验证：CDP 实测暴风 CMS：详情 9 集 → 点第一集 → 清单 15.5s 返回 →
    `readyState 4`、`currentTime` 递增、分片 200（修复前 30s 仍停在 `readyState 0`）；
    换源抽屉 +15.6s 收敛并出 5 行（暴风「当前」/ 非凡 / 虎牙 / 火狐 / 量子）。
  - **环境侧已知限制**：`b3.bdzybf22.com`（百度采集新 CDN）对宿主代理的 TLS 栈
    直接回 403「The region has been denied」，同一 URL 用 Node/OpenSSL 与浏览器栈
    均 200 —— 属 CDN 侧指纹 / 地域策略，不是代理链路故障；同源旧 CDN
    （`vod6.bdzybf11.com`）正常。
- **搜索 / 总览真分页（2026-10-21）**：
  - **之前**：每来源只取第 1 页；`loadMoreResults()` 只放大本地渲染窗口，不联网。
  - 契约：`SearchOptions.page`（缺省 1）+ `SourceSearchResult.hasMore`
    （来自 `Paged.next !== null`）；`searchSources` / `exploreSources` 都透传页码。
  - 游标：搜索结果与总览各一对 `pages`（来源 id → 已加载页）+ `more`（还有下一页
    的来源 id）。下一页按来源页码分组请求，避免总览里「后来才抓到的来源」错页。
  - 合并：同一来源的下一页**追加**到原结果并按 `workKey` 去重；出错（含 abort）
    不动游标，下次还能重试同一页；声明有下一页但这一页没有任何新作品（站点忽略
    `pg`）时按「到底」处理，避免无限翻同一页。
  - UI：哨兵 `#rh-more` 在「本地窗口还有」或「还有来源可翻页」时出现；
    联网期间显示「正在加载下一页」并禁用重复触发；总览不再固定 60 张 + 空闲帧追加，
    与搜索共用 `resultLimit`（30 条/段），`scheduleOverviewRest` / `queueIdle` /
    `cancelProgressiveRender` 一并删除。
  - 缓存：`OverviewCacheEntry.hasMore` 随首页一起落 KV；旧条目缺字段时按「可能还有」
    处理（首次翻页再收敛）。
  - 验证：CDP 实测单来源（暴风采集，关键词「爱」）搜索 20 → 40 → 60 张，
    每次「加载更多」都真正发出下一页请求；总览连续翻页 5000+ 张无重复、
    加载条与哨兵状态正确；`typecheck` / `build` 绿，SDK 82 + reader 13 测试全过。
- **真机验收结论与仓库刷新口径（2026-10-21）**：
  - 番剧嗅探、漫画章节、CMS 采集源、手机竖屏已在 Windows dev 真机验收通过；
    iOS 真机因环境限制暂缓（不做），Android 待 SDK / NDK 环境。
  - Venera 账号 / 收藏 / 分类按用户裁定**不做**。
  - **聚合仓库是导入时的快照**：`mergeRegistryEntries` 只合并当次导入的条目，
    发布新 `index.json` 后必须在「来源 → 导入规则」重新导入；导入只进聚合仓库，
    还要再点「安装」才会出现在已导入来源。发布后核对是否 110 条（含 6 个 `cms.*`），
    少于该数即说明本地还是旧快照。
- **iPad HD 适配（共享内核，2026-10-18）**：
  - `rh-reader` 新增 `≥768px` / `≥1024px 横屏` 断点：正文列宽与字号上浮、
    上下框左右留白加大；四边 padding 全部叠加 `env(safe-area-inset-*)`。
  - 触控：`pointer: coarse` 下按钮 ≥40px、进度条 26px；去点击高亮。
  - 图片：非条漫模式限制 `max-height`，整页图不超出可视区；条漫不变。
  - 外壳接入要求（viewport-fit=cover、横竖屏、Stage Manager）见 §8.5；
    Tauri 移动端外壳当时未接入，**2026-10-22 已暂时移出待办**。移动端公开仓库已同步（`ce80284`）。
  - 验收：浏览器按 1024×768 与 1366×1024 模拟断点，检查列宽 / 控件 / 安全区。
- **拼音检索（2026-10-22）**：
  - 宿主：`app/src-tauri/src/pinyin_bridge.rs` 复用既有 `pinyin` crate（零新增依赖），
    新增 `pinyin_encode(texts)` 命令；每批 ≤2000 条、单条取前 512 字符，
    输出 `{ full, initials }`（全拼按字拼接、汉字之间不空格，标点 / 空白当词边界；
    首字母只取汉字音节，ASCII 只进全拼）。
  - 桥：`__lunac_host.pinyin.encode(texts)` 是 apiVersion 3 的增补字段（只加不减、
    无权限门）；`app/src/plugins/host.ts` 与 SDK `host.ts` 都已声明，
    旧宿主 / 移动端缺该字段时插件**自动降级为纯子串匹配**。
  - 匹配：SDK `matchPinyin(query, entry)` 归一化后分 0（不中）/ 1（前缀）/ 2（包含）
    三档；查询与编码串都去掉非 `a-z0-9`，所以 `dou luo`、`dldl`、`斗罗` 都能命中
    《斗罗大陆》。
  - 插件：搜索建议与本地示例网格都接拼音；首屏先按子串出结果，`warmPinyin` 按
    200 条分批请求宿主，编码回来清 `itemsCache` / 重绘建议。排序上子串命中仍优先，
    拼音命中排在其后（`suggestionRank` 的 4 / 5 档）。
  - 验证：`cargo test --bins pinyin` 9 passed；SDK 87 条测试（含 `pinyin.test.ts` 5 条）
    全过；`typecheck` / `build` 绿，`dist/index.js` 1,238.02 kB / gzip 344.39 kB，
    已同步 dev 副本。
  - 如实记：拼音只作用于**本地已有标题**（示例 / 已缓存 / 当前搜索结果）；
    远程来源的首屏结果仍由站点搜索决定，不会因为拼音而多搜一轮。
- **B3 书架数据包导入 / 导出（2026-10-28）**：
  - **范围**：只带用户数据 —— 喜欢（`library`）、阅读进度 / 历史（`progress`）、书签（`bookmarks`）；
    **不带来源与聚合仓库**（来源能重新导入，且正文有许可边界，见 §9.6）。
  - **形状**：`{kind:"lunac-resource-hub-library", version:1, exportedAt, library, progress, bookmarks}`。
    `parseLibraryBundle` 对单条坏数据只丢弃该条（`library` 按 `workId`、书签按「章:页」去重），
    只有 `kind` / `version` 不对才整体报错，报错文案直接给用户看。
  - **导入是合并不是覆盖**：同一 `workId` 以 `updatedAt` 更晚者为准、书签取并集 ——
    「导出备份 → 又读几章 → 导回旧备份」不会把新进度冲掉。
  - **UI**：书架页右上「导出书架 / 导入书架」（`rh-btn` + 线性图标；新增 `export` 图标）；
    导出页文本框只读、`复制` 走 `navigator.clipboard`，失败就提示并全选让用户手动复制；
    导入页粘贴后「合并导入」的可用态随文本变化（只切 `disabled`，不整体重绘以保住光标）；
    Esc 回书架。沿用「导入规则」页的 `rh-import-view` / `rh-import-json` / `rh-actions`，未新造弹层。
  - **验证**：`npm run typecheck` / `npm test`（SDK 87 + reader 13）/ `npm run build` 全绿
    （`dist/index.js` 1,251.27 kB / gzip 348.56 kB）；纯函数另跑 17 条断言
    （roundtrip、坏 JSON / 坏 kind / 坏版本拒绝、坏条目丢弃、`workId` 与「章:页」去重、
    本地较新不被覆盖、备份较新会覆盖、书签并集与计数）全过；
    dev `Modules\resource-hub` 已按 §13.4 覆盖，产物核对含 `library-io` / `rh-io-json`。
  - **未做**：主窗内嵌与独立窗都还没手动点过（harness 里没有这条链路），属真机验收项。
- **A6 AI 技能（2026-10-28）**：
  - **落点**：`agent-templates/skills/resource-hub/SKILL.md`（唯一源）。`build-release.ps1` 已按既有链路把
    `agent-templates\skills\` 整棵铺到安装目录的 `skills\`；dev 手动同步到
    `app\src-tauri\target\debug\skills\resource-hub\`（与 code-review / debug 等内置技能同一路径）。
  - **修的两个硬伤**：① 旧稿**没有 frontmatter** ⇒ `skills.rs` 的 `parse_frontmatter` 取不到
    `name` / `description`，技能清单里描述为空，模型无从判断何时该用；② 旧稿要求先读
    `docs/resource-hub-design.md` 与 `resource-hub/README.md`，而装完的机器上没有这两份
    ⇒ 正文改为**自包含**（仓库文档只在开发时参考）。
  - **正文覆盖**：六个工具（`ResourceHubSearch` / `SourceValidate` / `SourceDiagnose` /
    `SourceInstall` / `ResourceHubOpen` 的入参与审批性质）、`{manifest, rule}` 形状与
    `SourceValidate` 的必填项、六段（`search` / `explore` / `chapters` / `pages` / `content` / `play`）、
    `Selector`（`css` / `xpath` / `jsonPath`）与 `FieldExtractor`（`attr` / `text` / `regex` / `template`）、
    占位符表（`@keyword` / `@page` / `@section` / `@workId` / `@workUrl` / `@chapterId` /
    `@chapterUrl` / `@sourceId`）、**与执行器对齐的字段名**（`title` / `id`·`url` / `author` /
    `cover` / `content`·`text` / `playUrl`）、`cms` 与 `sniff` 两个特例、错误码排错、
    Kazumi 只做格式转换与许可硬边界。
  - **如实记**：`paging` 目前只有清单层声明（`contract.ts` 有字段、执行器不读）⇒ 翻页只靠
    `@page` 占位符；接口返回 JSON 时必须写 `list.jsonPath`，执行器只有看到它才按 JSON 解析
    （否则把响应体当 HTML）。这两条已写进技能正文，避免模型生成「看起来对、跑起来空」的规则。
  - **验证**：按 `skills.rs` 的宽松 frontmatter 规则逐行核对 —— `---` 闭合、`name=resource-hub`、
    `description` 109 字（< `MAX_DESC_CHARS = 250`）；dev `skills\resource-hub\SKILL.md` 已落盘。
  - **未做**：没有真机端到端（让模型照技能生成一条规则并装成功），需要一次真实对话 +
    一个可访问的站点。
- **A7 对话入口（2026-10-27）**：
  - **工具**（`core-agent/src/resource_hub_tools.rs` 的 `ResourceHubOpen`）：入参 `query`（1..200 字）
    + 可选 `kind`（`comic` / `novel` / `anime` / `tv`，其它值直接报错）；输出
    `{ok, action:"search", plugin:"resource-hub", query, kind?}`。**纯数据**：不联网、不开窗、
    不写盘 ⇒ 免审批，只读档 / 单发 / 无人值守下都能用（工具不替用户弹窗）。
  - **聊天卡**（`app/src/main.ts`）：`ResourceHubOpen` 的结果 JSON 在工具卡里再渲染一块
    `.tool-rh-card`（标题 + 搜索词 + 类型 + 「打开阅读」）。点按钮才
    `invoke("open_plugin_window", {pluginId:"resource-hub", input: JSON.stringify({action:"search",query,kind})})`；
    开窗失败就地红字（`.tool-rh-err`），不弹原生 alert。识不出 JSON 时退回纯文本，不破坏结果卡。
  - **插件**（`apps/lunac-plugin/src/index.ts` + `app.ts`）：`parseInput` **只认显式 JSON**
    `{"action":"search",query,kind}`（2026-10-31 收窄，见下条）；挂载后交给
    `ResourceHubApp.applySearch(query, kind)` —— 落进 `#rh-q`、切回总览、收起建议 / 筛选抽屉，
    再走**同一套** `paint()+commitSearch()`（取消在途请求 / 复用建议 / 重置分页游标），不另开搜索路径。
    窗口与主窗内嵌面板共用 `mountWithPending()`。
  - **入口只认「显式请求」（2026-10-31 用户报障后收窄）**：此前宿主
    `openPluginWindow()` 在没给 `presetInput` 时默认把**搜索框原文**当 `input` 下发，插件又把
    任何非空字符串当搜索词 ⇒ 「打『斗罗大陆』→ 点阅读」直接落进一次搜索，而不是正常首页。
    用户口径：**搜索框里的内容不该直达阅读插件内部的搜索，阅读插件打开就正常打开。**
    两处一起改：① 宿主 `openPluginWindow` 只在调用方显式给 `presetInput` 时才带
    （转换插件带附件路径那一条），其余一律 `input: null`；② 插件 `parseInput` 不再接受普通
    字符串，认不出 JSON 就返回 `null`（= 正常打开）。AI 结果卡 / 作品卡走的 JSON 形态**不受影响**。
    插件版本 **0.2.2**，产物 **1,284,801 B**，dev `Modules\resource-hub` 已覆盖。
  - **冷启动排队（2026-10-27 补）**：全新实例 / 清过缓存时 `state.sources` 可能还是空，
    而宿主来源目录由 `hydrateHostSources()` 异步读回 —— 此时立刻搜只会得到 0 条。
    `applySearch` 因此在该情形下把请求排进 `queuedAutoSearch`，`hydrateHostSources` 收尾调
    `flushQueuedAutoSearch()` 补跑一次；来源本来就有（localStorage 缓存）则不入队、不重复搜。
  - **验证**：core-agent `cargo test --bins resource_hub` **6 passed**（新增
    `open_request_is_pure_data_and_validates_its_input`）；`npx tsc --noEmit` exit 0；
    resource-hub `npm run typecheck` / `npm test`（reader 13）/ `npm run build` exit 0
    （`dist/index.js` 1,240.33 kB / gzip 345.29 kB），dev `Modules\resource-hub` 已覆盖并核对产物含
    `applySearch` / `flushQueuedAutoSearch`。
  - **未做（二期）**：~~作品级结果卡~~ → **已实现（2026-10-28，见下条）**；
    剩余「点卡直接读第 N 章」；`resource-hub` 未走市场发布 ⇒ **未升版本号**，dev 直接覆盖。
  - **真机验收**：说「帮我找 XX 漫画」⇒ 模型调 `ResourceHubOpen` ⇒ 聊天出现卡片 ⇒ 点「打开阅读」
    ⇒ 阅读窗打开、搜索框已填好并自动出结果；再点一次同一张卡 ⇒ 已开的窗被前置并重新搜（不新开窗）。
    **另一条入口口径相反（2026-10-31）**：从主搜索框点「阅读」打开的是**正常首页**（不带搜索词）。
- **A7 二期 作品级结果卡（2026-10-28）**：
  - **回传链路（跨窗口）**：插件那一轮搜索完成 → `tauriInvoke("resource_hub_publish_results", {payload})`
    → 宿主 `commands::normalize_rh_results` 收敛（`query` 非空且 ≤200 字、`items` 必须是数组、
    截断到 20 条、整包 ≤64 KiB）→ `Emitter::emit("lunac-resource-hub-results")` **广播给所有窗口**
    → 各窗口 `main.ts` 的监听按 `dataset.rhQuery`（+ `rhKind`）找回**同 query** 的
    `ResourceHubOpen` 卡片，把前 8 条填进 `.tool-rh-results`；没有这张卡的窗口静默丢弃。
  - **只回传 AI 发起的那一轮**：`applySearch` 置 `publishPending`，`runRemoteSearch`（含「复用建议」
    那条分支）消费一次即清 —— 用户自己手搜不会往聊天里刷结果；「加载更多」也不回传。
  - **点条目 → 直接进该作品详情**：`open_plugin_window` 带
    `{action:"open", work:{sourceId,workId,kind,title,author}}`；插件 `parseOpen` 认这条入参，
    `openPublishedWork()` 用最少字段重建 `Work` 塞进 `remoteWorks`，再走与搜索结果卡**完全相同**的
    `view = detail` 路径（章节由详情页自取）。来源已卸载 ⇒ 退回按标题搜索，不给空白详情页。
  - **边界**：回传不含封面 / 章节 / 凭据；不落盘、不开窗、不联网（宿主只做转发）；
    卡片随聊天重渲染会丢（结果不持久化，属预期）。
  - **验证**：`cargo test --bins resource_hub_results` **2 passed**（超上限只截断不整体失败；
    非 JSON / 非对象 / 缺 query / 空白 query / items 非数组 / query 201 字 六种坏形状全拒）；
    `cargo check --bins` OK；`npx tsc --noEmit` OK；resource-hub `typecheck` / `build` OK
    （`dist/index.js` 1,254.90 kB / gzip 349.86 kB），dev `Modules\resource-hub` 已覆盖并核对产物含
    `resource_hub_publish_results` / `openPublishedWork`。
  - **未做**：真机点检（尚未跑「说一句话 → 点卡 → 回传 → 点条目进详情」全链路）；
    「点卡直接读第 N 章」（要连章节列表一起回传，属更细一层）。
- **uzVideo sniffer（兼容层剩余项，2026-10-28）**：
  - **问题**：type 101 的 `getVideoPlayUrl` 有时返回**播放页地址**（如 `https://play.x.com/play/1`），
    此前一律按扩展名猜成 `mp4` ⇒ 播放器只会报「不支持的格式」，用户看到的是「有地址但播不了」。
  - **做法**：`video-source.ts` 的 `play()` 收尾加一层 `sniffPlayPages()` —— 地址不是直链
    （判据 `isMediaUrl`，本次由 `executor.ts` 私有函数改为**导出**，与 CMS / `rule.sniff` 共用同一份）
    且宿主提供 `sniff` 时，交给 `host.sniff.media(url, {timeoutMs})` 换回真直链，按扩展名重新定
    `hls` / `dash` / `mp4`，并沿用该线路的请求头。一次 `play()` 最多嗅探 **3** 个播放页
    （隐藏 WebView 很贵，多线路时只挑前几个试）。
  - **降级**：旧宿主没有 `sniff`、清单未声明 `webview.sniff`、或嗅探不到 —— **原样保留**原地址；
    这是纯增强，不改既有判定，不让原本能播的来源回归。
  - **验证**：SDK 测试 87 → **90 条**（+3：播放页地址嗅探成 hls / mp4、嗅探为空时保留原地址、
    直链不触发嗅探）；`typecheck` / `build` 绿（`dist/index.js` 1,255.50 kB / gzip 350.01 kB），
    dev `Modules\resource-hub` 已覆盖。
  - **未做**：真机验证需要一条真实的「播放页」来源（且清单带 `webview.sniff`）；
    剩余兼容项 = BaseCode1 嗅探模板、网盘工具（type 300，需 sidecar）、
    弹幕（type 400，播放器没有弹幕层）、加密 `codeID` 源。
- **uzVideo 扩展数据类（真实脚本跑通，2026-10-28）**：
  - **起因**：用户没有真实来源，按要求去上游仓库取证。`YYDS678/uzVideo-extensions` 的
    `vod/js/*.js`（type 100/101）与 `core/core/uzVideo.js`（公开数据类型定义）给出了真实形态。
  - **问题**：线上扩展不拼对象字面量，而是 `new RepVideoList()` / `new VideoDetail()` 后逐字段赋值，
    `req` 回来用 `$(…)[0].attribs.href`、`$(…)[0].children[0].data` 这类 cheerio 原生写法，
    还会 `console.log`。此前 shim 只提供 `req` / `cheerio` / `UZUtils` ⇒ 真实脚本第一行就
    `ReferenceError: RepVideoList is not defined`，仓库里已登记的 45 条 uzVideo 来源实际都跑不起来。
  - **做法**（`comic-shim.ts`，只有默认值、不含上游逻辑）：
    - 补 `FilterLabel` / `FilterTitle` / `VideoClass` / `VideoSubclass` / `VideoDetail` /
      `VodDanMu` / `RepVideoClassList` / `RepVideoSubclassList` / `RepVideoList` /
      `RepVideoDetail` / `RepVideoPlayUrl` / `UZArgs` / `UZSubclassVideoListArgs`；
    - 补空 `console`（QuickJS 没有）；
    - cheerio 补下标访问：`$(…)[0]` 返回只读节点视图（`.attribs` / `.name` / `.children` /
      `.data`，子节点懒展开），`$()` 再按 `.node` 包回集合；
    - `req` 的响应类型判定对齐上游：`application/json` → 对象，`text/*` → 字符串，
      `octet-stream` → bytes，其余先试 JSON 再退字符串（MacCMS 采集站常回 `text/html`）；
    - 分类浏览按上游 `UZArgs` 约定把分类标识放进 `args.url`（`type_id` 保留给旧脚本）。
  - **验证**：SDK **92 条**（+2：真实形态脚本的 search / detail / chapters / play 全通、
    分类浏览读 `args.url`）；`typecheck` / `build` 绿（`dist/index.js` 1,259.26 kB / gzip 351.35 kB），
    dev `Modules\resource-hub` 已覆盖。另用仓库外的临时脚本把**上游真实脚本**
    `vod/js/Cai_cmsUser.js` 直接喂给 SDK（mock 宿主回 MacCMS 形状 + `text/html`），
    搜索 / 详情 / 剧集 / 播放 / 总览五条链路全部返回预期数据。
  - **未做（2026-10-29 已从待办删除、不再跟踪）**：BaseCode1 模板 —— 正文只有 `webSite` /
    `searchUrl` / `videoDetailPage` / `videoPlayPage` / `firstClass` 几个常量 + `//#BaseCode1#`，
    抓取逻辑在 uz 内置代码里且不公开，照猜只会给出「能跑但抓空」的源 ⇒ 维持可读 `unsupported`；
    type 300（网盘工具）、type 400（弹幕）、加密 `codeID` 源同理不接。
- **uzVideo 不兼容源导入前置拦截（2026-10-28）**：
  - **问题**：registry 里那批 uzVideo 源混有 BaseCode1 与加密 `codeID` 形态，导入后要等用户首次搜索
    才报 `unsupported`，观感是「装了却坏了」，而且无从知道为什么。
  - **做法**：SDK 新增 `videoUnsupportedReason(script)` → `basecode1` / `encrypted` / `unknown` /
    `null`，只读正文（不联网、不执行脚本）；插件 `recordFromSourceUrl` 在拿到正文、
    构造清单**之前**判一次，命中即抛 `unsupported` 并带可读原因。判定细节：
    `//#BaseCode1#` 算模板形态；`//@codeID:` **有非空值**才算加密（模板占位是空值，不能误杀）；
    其余连 `looksLikeVideo` 都不满足的记 `unknown`。
  - **顺带**：一键安装的失败提示从「只列名字」改成「名字：原因」（最多 3 条），否则用户只知道失败数。
  - **验证**：SDK **93 条**（+1：正常源 / BaseCode1 / 加密 / 空 `codeID` 占位 / 未知形态 五种输入）；
    `typecheck` / `build` 绿（`dist/index.js` 1,260.13 kB / gzip 351.73 kB），dev `Modules\resource-hub` 已覆盖。
  - **未做**：真机走一遍「一键安装」看失败提示；漫画侧没有同类形态，未加拦截。
- **播放器退出后的后台拉流（2026-10-28，报障修复）**：
  - **现象**：视频播放中点「返回剧集」、关闭内嵌插件面板、关闭插件窗，三种路径都会继续出声 /
    跑流量。
  - **根因三处**：① `index.ts` 的 `detach()` 直接丢 `app` 引用，从不销毁 —— 播放器、Hls 实例、
    定时器全部存活；② `onExit` 只切 `view` 再 `paint()`，而 `paint()` 走 rAF + 120ms 兜底，
    窗口隐藏时兜底被节流，期间仍在拉流；③ 宿主 `setPluginBar(null)` 只改样式、**不移除 iframe**，
    面板「关了」但 iframe 里的 `<video>` 照常解码 + 请求 MSE 分片。
  - **做法**：`player.ts` 新增 `teardownVideo()`（`pause()` → `srcObject=null` →
    去 `src` → `load()` 中止在途请求 → 清字幕轨），`destroy()` 改成「先停元素、再拆 hls」；
    并注册 `pagehide`（iframe 被移除 / 窗口关闭时同步 destroy）与 `visibilitychange`
    （`hidden` 时停流，回来可继续播）。`app.ts` 新增 `ResourceHubApp.destroy()`，
    收掉播放器 / 阅读器 / 搜索 `AbortController` / 三个 observer / 五个定时器；
    `onExit` 立刻 destroy 并清 `pendingVideo`。`index.ts` 的 `detach()` 与「换 root 重新挂载」
    都先 `app.destroy()`。`main.ts` 新增 `unmountIframePlugin()`，`setPluginBar(null)` 时
    dispose 信道并移除 `iframe.plugin-iframe`。
  - **验证**：`typecheck` 双端绿；插件 build `1,261.86 kB` / gzip `352.15 kB`，
    dev `Modules\resource-hub` 已覆盖；SDK 93 + reader 13 全过；`npx vite build` 通过。
  - **未做**：真机点检三条路径（播放器返回 / 收起内嵌面板 / 关闭插件窗）与「最小化后暂停、
    恢复窗口继续播」。
- **百合会 loadEp 复核（2026-10-28）**：
  - **背景**：2026-10-21 记过「`百合会` 单章 `loadEp` 返回 `parse`，待观察」。这次按上游真实脚本
    重新验证，结论是**当前代码已不成立** —— 当时的两轮宿主修复（`plugin_net` 共享 Client、
    `__lunac_request` 解包 `headers.headers`）顺带解决了它，只是没回填结论。
  - **取证方式**（不落库，脚本只在 `%TEMP%`）：从上游 `venera_comic_source` 取 `baihehui.js`
    （25 KB，v1.0.0），用 Node 把 `createComicSource` 跑起来；沙箱里的 `host.request` 是同步的，
    Node 没有同步 fetch ⇒ 用 `curl.exe` 做同步替身，网络与解析全走真实站点。
  - **实测**：`detail` = 「高嶺之花」；`chapters` = 1 条（`data-key=22`）；
    `pages` = **15 张**（与页面 `li.last` 的 `data-page=14` 一致）；共 18 次请求 / 6.5 s。
    另用纯 HTTP 逐页探测过：15 页全部 200、全部命中 `img#imgPic`、总耗时 4.4 s
    ⇒ 既不是反爬，也不是超时。
  - **顺带修掉的真差异**：上游 `ComicDetails.tags` 允许「分组 → 标签列表」对象
    （百合会返回 `{作者:[…], 更新:[…], 标签:[…]}`），沙箱 `__lunac_toWork` 只透传数组 ⇒
    详情页标签整块丢失。改为 `__lunac_tags()`：数组原样、对象拍平成 `分组：值`，空组丢弃。
  - **验证**：SDK **94 条**（+1：分组对象标签），reader 13 条；`typecheck` / `build` 绿，
    dev `Modules\resource-hub` 已覆盖。真实脚本复核后 `tags` =
    `["作者：透村","更新：：2017-11-10","标签：翻页漫画"]`（双冒号是上游 `replace` 没吃干净，属数据本身）。
  - **未做**：真机（QuickJS + plugin_net）点同一章；`更新：：` 这类上游脏值不做兜底清理。
- **uzVideo `Crypto` 实现为 CryptoJS 兼容层（2026-10-29）**：
  - **取证**：上游 `core/core/uz3lib.js` 里 `const Crypto = createCryptoJS()`、`const Encrypt =
    loadJSEncrypt()` —— 也就是说 `Crypto` 是**整个 CryptoJS**，不是我们原先猜的
    `md5/sha256` 小对象；`Encrypt` 是 JSEncrypt（RSA）。拉了整仓（98 个 js）统计：
    **16 个 vod 脚本**用 `Crypto`，常用的只有 `Crypto.MD5` / `SHA1` / `SHA256`、
    `enc.Utf8/Hex/Base64`、`lib.WordArray.create/random`、`AES.encrypt/decrypt`
    （CBC / ECB × Pkcs7 / NoPadding）；`Encrypt` 只在 `panTools`（type 300）里出现，**vod 0 处**。
  - **做法**：视频沙箱提供 CryptoJS 子集 —— `WordArray`（`words` / `sigBytes` /
    `toString(encoder)` / `concat` / `clamp`）、`enc.Utf8|Hex|Base64`、`lib.WordArray.create|random`、
    `MD5 / SHA1 / SHA256 / SHA512 / HmacSHA256`、`AES.encrypt|decrypt`（CBC / ECB，
    Pkcs7 自动补位 / 去位，NoPadding 原样）。底层复用已有纯 JS 摘要与新增的 AES 加密
    （同时把漫画侧 `Convert.encryptAesCbc` / `encryptAesEcb` / `sha1` 从抛错改成实现）。
  - **未做**：`Encrypt`（JSEncrypt / RSA）—— 只被网盘工具使用，type 300 已从待办删除；
    `Crypto.AES.encrypt` 传入**字符串密钥**时按 UTF-8 处理，不实现 CryptoJS 的 OpenSSL
    KDF（抽样脚本都用 `Utf8.parse` 传 WordArray）。
  - **验证**：SDK **97 条**（+1 覆盖 MD5 / SHA1 / SHA256、Base64、NIST AES-128-CBC 向量
    `7649abac8119b246cee98e9b12e9197d`、CBC/ECB 的 Pkcs7 往返）；build 绿。
- **Cookie 注入媒体代理（2026-10-29）**：
  - **背景**：来源设置里的 Cookie 只随 `net.request` 发送；`<img>` / `<video>` 走的是
    `media.proxy`，之前只转发 Referer / UA。需要登录态才出图的站点（封面 403、播放 403）
    明明来源设置里填了 Cookie 也不生效。
  - **做法**：代理协议加第四段 `&c=<cookie>` —— `app.ts` 的 `proxyMediaUrl()` 从
    `manifest.headers` 取 `Cookie` / `cookie` 交给 `media.proxy(url, { referer, userAgent, cookie })`；
    宿主 `plugin-window.ts` 拼 `c=`、`plugin_media::parse_options()` 解出第三元组、
    `media_proxy::fetch()` 透传给 helper、`media-proxy` 落成 `Cookie` 请求头。
    与 Referer / UA 同一条链，只做 `safe_header()`（拒绝 CR / LF，防头注入）。
  - **形态**：只加查询参数，不改 helper 的 HTTP 协议形状；`apiVersion` 仍为 3
    （新字段是可选参数，旧插件不传即行为不变）。
  - **验证**：`cargo check --bins` 绿；`cargo test --bin lunac parses` 5 条通过
    （`plugin_media::tests::parses_referer_user_agent_and_cookie_options` 覆盖
    `c=sid%3D1%3B%20theme%3Ddark` → `sid=1; theme=dark`）；`cargo build`（media-proxy）绿；
    `npx tsc --noEmit` 绿；SDK **97 条** + reader 13 条全过；插件 `dist/index.js` 1,273,066 B，
    dev `Modules\resource-hub` 已覆盖。
  - **未做**：不自动管理 Cookie（无 Cookie Jar / 登录模拟）—— 仍由用户在来源设置里填写；
    Cookie 会出现在代理 URL 里（仅本机 WebView 内可见，不进日志）。
- **远程漫画阅读时整块变白 / 像卡死（2026-10-29，报障）**：
  - **现场（CDP 实测，不是性能问题）**：插件窗主线程 ping 1–3 ms；`.rh-app` 在，但 `.rh-main`
    只有 **32 字符**（`<div id="rh-reader-mount"></div>`），`.rh-reader` 根本不存在
    ⇒ 面板白板，用户看到的就是「卡死」。
  - **根因**：`paintNow()` **每次重绘**都会销毁并重挂阅读器，而 `mountReaderView()` 第一次挂载时
    把 `pendingRemote` **置空**；远程作品没有演示数据 ⇒ 第二次进来直接 `return`，只剩空 mount 点。
    触发条件是阅读期间的**任何一次** `paint()`（总览后台刷新完成、警告、详情补全回调等）。
  - **修法**：① `pendingRemote` 不再在挂载时清空（按 `workKey` 复用）；② `readerSession` 增加
    `chapterIndex` / `pageIndex`，由 `persistReaderProgress` 与 `onChapterChange` 持续更新，
    重绘后按原位挂回；③ `pendingRemote` 缺失但会话还在 ⇒ 按会话重建 lazy 章节表再挂；
    ④ 两者都没有 ⇒ 退回详情页（不再留白板）；`destroy()` 清空这两个字段。
  - **验证**：`tsc --noEmit` 绿；插件 build **1,275,334 B**，dev `Modules\resource-hub` 已覆盖。
  - **未做**：真机复测（读一章期间触发后台刷新 / 翻章，确认不再白板且位置不丢）。
- **退出后仍有声音：脱离 DOM 的 `<video>` 仍在解码（2026-10-29，报障）**：
  - **取证（CDP `Media` 域，不靠猜）**：插件页 `document.querySelectorAll("video").length === 0`，
    但浏览器媒体栈里仍有 **5 个 media player**（其中一个 `kVideoDecoderName = D3D11VideoDecoder`、
    `kAudioDecoderName = FFmpegAudioDecoder`），3 秒内 **0 条网络请求**（说明放的是已缓冲的数据）。
    ⇒ 有 `<video>` **脱离了 DOM 却仍在解码**：浏览器不会回收仍在播的媒体元素。
  - **修法（两道）**：`player.ts` 加**模块级登记表** `liveVideos` + `stopElement()` / `stopAllPlayers()`；
    三处兜底调用 —— `paintNow()` 整树重建前、`ResourceHubApp.destroy()`、`player.destroy()`
    （`teardownVideo()` 也复用 `stopElement()`）。这样无论哪条路径忘了 destroy，都不再有游离播放器。
  - **顺带**：8s「一帧都没出」看门狗加了一次 +5s 复查 —— 慢源在 8–13s 之间出首帧的话自动撤掉
    「编码不受支持」提示（用户已点过「复制直链」则不撤），避免误伤。
  - **验证**：`tsc --noEmit` 绿；插件 build **1,273,997 B**，dev `Modules\resource-hub` 已覆盖。
  - **未做**：真机复测（重启 dev / 重开插件窗后跑 §测试 21.4）。
- **「有声音、没画面」的兜底（2026-10-29，报障）**：
  - **现象**：某条线路能出声，画面一直停在「正在加载视频…」/ 浏览器自带转圈。
  - **根因**：**视频轨**编码不受支持（HLS / MP4 里的 HEVC、AV1…），而**音频轨**是 AAC ⇒
    `<video>` 照常解码出声、`currentTime` 也在走，但 `video.error` **始终不触发**
    ⇒ 上一版只挂在 `error` 上的 `codecUnsupported` 出口（2026-10-29 那轮 MKV 收敛）兜不住。
  - **做法**：`player.ts` 新增「一帧都没出」看门狗 —— `playing` 后 8s 检查一次，
    若**没报错、没暂停、`currentTime ≥ 1.2s`，但 `videoWidth === 0` 且
    `getVideoPlaybackQuality().totalVideoFrames === 0`**，直接复用「编码不受支持（MKV / HEVC），
    换线路 / 换源 + 复制直链」这条出口。`teardownVideo()` / `destroy()` 会取消计时器。
  - **边界**：真正**纯音频**的线路也会命中（同样提示「换线路」）；自动播放被拦（`paused`）时不判定，
    避免把「用户没点播放」误报成编码问题。
  - **验证**：`tsc --noEmit` 绿；插件 build 1,273,569 B、dev `Modules\resource-hub` 已覆盖。
  - **未做**：真机复现那条线路（需要原来源 + 8s 观察）；没有把它细分成「音频可播 / 视频不可播」
    的独立文案。
- **MKV / HEVC 播放策略（2026-10-29，四轮收敛，最终不含解码器）**：
  - **第一轮（已撤回）**：识别 `mkv` → 交外部播放器。探测只查 VLC / PotPlayer / mpv 三个固定
    路径；干净 Windows 上都不存在，兜底 `cmd /c start` 命中 **http 协议关联（默认浏览器）**
    而不是 `.mkv` 文件关联，实际等于「下载一个文件」。
  - **第二轮（保留）**：RSP 不新增线路类型 —— `playbackTypeFromUrl()` 只判 `.m3u8` → hls /
    `.mpd` → dash，其余（含 `.mkv`）按 mp4；`isMediaUrl()` 仍把 `.mkv` 当直链，不当播放页嗅探。
    播放器不再对 MKV 特殊处理，统一给 `<video>` 试播（Chromium 对 Matroska 有部分解复用能力，
    **H.264 / AAC 的 MKV 多数能播**，一刀切拒绝会误伤能看的线路）。
  - **第三轮（已撤回）**：短暂接过「本机 mpv 独立窗口」兜底（locator + 命名管道 IPC +
    `plugin_mpv_*` 四个命令 + 1s 进度轮询）。
  - **采样（决策依据）**：本机 109 条已装来源里视频类 67 条；6 条可达的 CMS 采集 API 首页共
    15,465 个播放地址 —— **`.m3u8` 12,111 / `.mp4` 3,354 / `.mkv` 0**。需要 mpv 的是「编码」
    而不是「容器」（HLS/MP4 里的 HEVC、AC3），属长尾。
  - **第四轮（最终，用户裁定）**：撤掉 mpv 整套（`mpv.rs`、4 个宿主命令、插件按钮与轮询），
    只保留「试播 + 失败提示 + 复制直链」：`<video>` 的 `error.code` 为 3（DECODE）/ 4
    （SRC_NOT_SUPPORTED）、hls.js fatal 或 `Hls.isSupported()` 为假时，提示
    「该线路编码不受支持（MKV / HEVC），请换线路或换源」，并显示「复制直链」
    （`navigator.clipboard`，被权限拦时退回 `execCommand`）。
  - **删除清单**：`plugin_open_external_media` / `external_player_candidates` /
    `media_header_pair`、`media_proxy::local_url`、`media-proxy` 的 `stream=1` 直通、
    `app/src-tauri/src/mpv.rs`、`plugin_mpv_open/state/command/close`、插件 `onExternal` /
    `onDecodeFallback` / `onPlaybackChanged` 与相关文案、`Playback.type` 的 `"mkv"`。
  - **验证**：SDK **97 条**、reader 13 条全过；`npm run typecheck`、`cargo check --bins`、
    `cargo build`（media-proxy）全绿；插件 `dist/index.js` 1,264,130 B，dev
    `Modules\resource-hub` 已覆盖。
  - **未做（明确不排期）**：内嵌解码。已确认 libmpv 的画面只能渲染到宿主提供的原生表面
    （Render API），**不能「只解码、把帧交给 JS 播放器」**；WebCodecs 的 HEVC 依赖系统扩展
    与硬件。两者都要重做渲染层，收益与 MKV 长尾不匹配。
- **加密图源：章节图片加载不出来（2026-10-29）**：
  - **现象**：「许多漫画资源在具体章节的图片加载不出来」；阅读器里的 `<img>` 全是坏图
    （`naturalWidth === 0`）。
  - **取证**：CDP 抓现场 `imgs:1 / broken:1`；把那张图用 `curl.exe` 直连是
    `200 image/jpeg 224720`，但首 4 字节是 `33 E9 0C ED` —— 既不是 JPEG 的 `FF D8 FF`，
    也不是 WebP 的 `RIFF`。按 `comic.manwaba` 脚本自己声明的 key / iv 解一遍得到
    `RIFF…WEBP`，尾部 10 个 `0x0A`（PKCS#7 填充）⇒ **上游发的是 AES-256-CBC 密文**，
    代理与宿主都没坏。
  - **根因**：Venera 形态的 `comic.onImageLoad(url, comicId, epId)` /
    `comic.onThumbnailLoad(url)` 返回 `{ url, headers, onResponse }`，而兼容层 `pages`
    只取 `url`：逐页 headers 与 `onResponse` 全被丢掉 ⇒ 32 条来源的图片 headers 不生效、
    6 条加密图源（manwaba / manwang / mojoin_v2 / manga51 / gfmhapp / ccc）必然坏图。
  - **做法**：`comic-shim` 的 `pages` 补 `headers`（`loadEp.headers` + `onImageLoad.headers`）
    与 `transform` 标记；新增沙箱方法 `image`（源自己取字节 → `onResponse` → base64）。
    `ComicPage` 加 `transform?`、`ResourceSource` 加 `imageData?`；插件
    `resolveComicImageUrls()` 对 `transform` 页并发 3 取明文并换成 `blob:` URL
    （`destroy()` 统一 `revokeObjectURL`），其余页把逐页 headers 交给媒体代理。
    **代价如实**：加密页要整章先过一遍 JS（一页一次 `engine.eval`），且受沙箱
    「结果 ≤1 MiB」限制 —— base64 后超上限的大图会退回代理地址（仍是坏图）。
  - **封面（同一条链的另一半，2026-10-30 补上）**：`onThumbnailLoad` 走同一个沙箱 `image`
    方法（不传 chapterId 即分流到缩略图配置）。插件侧只在封面**加载失败**时才动手：
    渲染时写 `data-thumb-source` / `data-thumb-url`（原地址），在 root 上**捕获阶段**听
    `error`（`error` 不冒泡），换 `blob:` URL 后重挂；解不出来就保持坏图、不弹提示。
    受沙箱「结果 ≤1 MiB」限制，超限封面仍退回坏图。
  - **验证**：用**真实** `comic.manwaba` 脚本 + 真实密文离线跑通 → 224,704 B `RIFF…WEBP`；
    SDK 测试 **98 条**全过（新增「onImageLoad 的 headers / onResponse 落进 pages 与
    imageData」+「无 chapterId ⇒ onThumbnailLoad」）；SDK 与插件 `typecheck` 绿；
    插件产物 **1,284,354 B** 已覆盖 dev `Modules\resource-hub`。
  - **未做**：真机看一次加密图源的搜索结果 / 书架封面（§待办 3-20）。
- **音画同起：先出声、1~3s 才出画面（2026-10-30 报障）**：
  - **现象**：部分线路「先听到声音，画面还卡在正在加载」，用户侧读作播放器坏了。
  - **根因**：`video.play()` 解到**音频**就出声，**视频轨**要等解码器初始化 + 首个关键帧；
    两轨不是同时就绪，元数据到位（`loadedmetadata`）也不代表能出帧。
  - **做法**：起播前把 `<video>` 置 `muted`，等**第一帧真的呈现**
    （`requestVideoFrameCallback`，回退 `loadeddata` + `videoWidth > 0`）再恢复用户原来的
    静音状态；8s 仍未出帧就放手（纯音频流不会被永久静音），画面的事交给已有的
    「8s 无帧 ⇒ 编码不支持」看门狗。「正在加载视频…」也改由**首帧到达**撤掉，
    不再由 `loadedmetadata` 撤。
  - **未做**：不等到首帧就 seek 回 0 重播（会让弱网重新缓冲）；接受「前 1~3s 静音」。
  - **二次报障（2026-10-31）**：用户反馈「资源库还是先出声音再出画面」，并给出两个猜测：
    广告 / dev 里的插件不是最新。**核对结果**：dev 的 `Modules\resource-hub\index.js`（10-30 18:57）
    确实**已含**首帧闸门（不是纯陈旧构建），所以问题在闸门本身，两处：
    ① **一帧 ≠ 有画面**：解码器起播常先呈现一帧**起始黑帧**，`requestVideoFrameCallback`
       对它照常回调 ⇒ 一帧就放声 = 黑屏出声（用户看到的仍是「先出声后出画面」）；
    ② **8s 兜底从 `load()` 起算**：慢源拉清单 / 首个分片花掉 7–8s 时，闸门在真正开播 1s 后
       就到期 ⇒ 等于没设。
  - **加固（2026-10-31）**：判据改成「**两帧**已呈现 + `videoWidth > 0`」；帧数由**自己的回调计数**
    得到（不用 `metadata.presentedFrames` —— 它按元素生命周期累计，换线路后一上来就超阈值）；
    8s 兜底改挂在 `playing` 上（`startGateTimer`），`load()` 只负责静音。每呈现一帧重挂一次
    回调，`cancelFrameGate()` 里一并 `cancelVideoFrameCallback`，换线路 / 退出不残留。
    插件版本 **0.2.1**（设置 → 插件可据此确认跑的是新构建），产物 **1,284,836 B** 已覆盖
    dev `Modules\resource-hub`（`typecheck` + `build` 均 exit 0）。
  - **仍未做**：没有运行期证据能区分「起始黑帧」与「源自带前置广告」—— 若加固后仍复现，
    下一步是在 `player.ts` 记 `presentedFrames / videoWidth / currentTime` 的起播时间线
    （临时日志），而不是继续猜。
- **重复播放：同一段声音隔约 0.5s 又响一次（2026-10-31 报障，每条线路都会）**：
  - **根因**：`app.ts` 的 `paintNow()` 每次整树重建，并在 `view.kind === "player"` 时**重新
    `mountPlayerView()`** —— 旧 `<video>` 被 destroy、新 `<video>` 从 `pendingVideo.initialPosition`
    重新起播。而 `paint()` 的触发点并不只有用户操作：开插件时 `hydrateHostSources()` /
    `refreshOverview()` 都是异步的，收尾各 `paint()` 一次 —— 用户「开插件 → 点作品 → 点集数」
    够快时，这次迟到的重绘就落在播放开始后约 0.5s，把视频**从头重放一遍**。
    听感 = 「两段音轨、前后差 0.5s」；换线路也一样，因为触发点是重绘时机而不是流本身
    （这也解释了为什么**每条线路都会**）。
  - **修法**：`paintNow()` 在「播放器已挂载 + 挂着的正是这一集」时**直接返回，不重绘**。
    播放器界面自包含（进度 / 换线 / 字幕都在 `player.ts` 里），没有非重绘不可的状态；
    退出时 `onExit` 先 destroy 再把 view 改成 detail ⇒ 下一次 paint 正常重建。
    判据带上 `workId + chapterId`：将来若有「播放中换集」的路径，重绘必须放行，不被这条卡死。
  - **兜底**：`mountPlayer()` 入口先 `stopAllPlayers()` —— 任何一条漏了 `destroy()` 的路径都
    不该留下第二个 `<video>` 继续出声（正常路径由 `paintNow` 先停，这里只是空循环）。
  - **验证**：`resource-hub` `typecheck` / `build` exit 0（SDK 98 + reader 13 测试全过）；
    插件版本 **0.2.3**、产物 **1,285,043 B**，已覆盖 dev `Modules\resource-hub`。
  - **仍未做**：没有在真机上量过「迟到的重绘」到底落在第几毫秒；若 0.2.3 仍复现，
    下一步是把 `paintNow` 的每次重建记进插件日志（连 `view.kind` 与播放位置），不再靠猜。
- **退出后仍在后台播放（2026-10-31 报障，用户原话「不管哪个退出音频都会出现后台播放」）**：
  - **根因**：宿主**整段替换结果区**时插件实例还活着。最典型的一条是「播放中回主窗再点一次
    阅读」：宿主发现同名插件窗已开着 ⇒ 走 `plugin-window-input` ⇒ 插件窗 `render()` 直接
    `root.innerHTML = 卡片`。因为**插件 id 没变**，`render()` 开头那句
    `if (currentPluginId !== pluginId) detachPluginListeners(...)` 不会触发 ⇒ `app.destroy()`
    从未跑过；播放器那棵 DOM 已被丢掉，`<video>` 仍被模块级 `liveVideos` 引用着 ⇒
    浏览器不会因为「元素脱离文档」而停声。
    更糟的是 0.2.3 的 `paintNow` 早退判据只看「handle / pendingVideo / session」——
    宿主换掉 DOM 后 `app.mount()` 走到这里仍然早退 ⇒ 界面回到卡片、声音继续，看起来
    就是「退出了还在后台播」。
  - **修法一（判据补一条）**：`PlayerHandle` 新增 `isAttached()`（= `video.isConnected`）。
    `paintNow` 的早退**额外要求播放器真的还在文档里**；宿主把 DOM 整段换掉时必然放行重建，
    而重建的第一步就是 `destroy()` 旧播放器 ⇒ 声音随即停。
  - **修法二（播放器层看门狗）**：`player.ts` 的 `timeupdate`（播放中约 4Hz）里判
    `!video.isConnected` ⇒ 自己 `handle.destroy()`。最迟 250ms 停声，**不赌宿主每条退出路径
    都记得调 `destroy()`** —— 这条同时兜住主窗收面板 / 未来新增的宿主清理路径。
  - **验证**：`resource-hub` `typecheck` / `build` exit 0（SDK 98 + reader 13 测试全过）；
    插件版本 **0.2.4**、产物 **1,285,208 B**，已覆盖 dev `Modules\resource-hub`。
  - **仍未做**：没有在真机上量过「重开插件」这条路径从 DOM 被换到停声的实际延迟；
    若仍复现，按 §测试 21.5 第 8 步先确认跑的是 0.2.4。
- **已知限制**：远程来源支持跨章按需加载；视频已按 `play` 规则播放，
  MKV 按直链试播、HEVC 只提示换线路（2026-10-29，见上条）；漫画 / 视频主链路已兼容。
- **插件框架（前置）**：E.0 vitest + 12 条协议测试；清单 `provides` / `render` 字段；
  `net.fetch` 宿主桥（`apiVersion 3`）；iframe 信道与自定义协议已接悬浮窗与主窗内嵌；
  `provides` 解析已覆盖窗口 / sidecar / 依赖安装 / `local_model_env()`；外观提示与
  插件包 `sha256`（索引 + 安装前校验 + 发布脚本）已落地；两个常驻锚点
  `chat-sidebar` / `settings-section` 已施工（`render.slots`，只支持 iframe）；
  工具链授权（`dev` 段 + `config\plugin-toolchain.json` 指纹账本）与每 sidecar 独立
  Job Object 限额（`memoryLimitMiB` / `processLimit`）已落地。
  裁剪模式与阅读模式的成熟方案仍后置。
- **下一步（用户裁定）**：**真机验收**。全部剩余项已收敛成一条路线 ——
  `docs/testing.md` **§测试 21 资源库一次性验收地图（2026-10-29）**（iframe / 两个锚点 / 工具链、
  Cookie 注入、播放器三条退出路径、HEVC 兜底、一键安装提示、uzVideo sniffer、A7 一 / 二期、
  A6 技能端到端、B3 书架导入导出、百合会 loadEp），走完即只剩出包。
  ~~完成后接入 Tauri 移动端外壳~~ —— **2026-10-22 用户裁定：移动端外壳暂时移出待办**，
  共享内核（含 iPad HD 断点与安全区）保留，恢复时按 §8.5 外壳要求接。
  跨章按需加载、来源缓存 / 限速 / `AbortSignal`、视频播放、EPUB 阅读、
  JS 源运行时（`createSource(rsp)`）、Venera 兼容层、uzVideo type 101 兼容层、
  iPad HD 共享内核适配均已完成（2026-10-18）。

---

## 附录 A：来源适配器方法映射

| RSP | Venera `ComicSource` | uzVideo `WebApiBase` | Kazumi 规则 |
|---|---|---|---|
| `search` | `search.load(keyword, options, page)` | `searchVideo(key)` | `searchURL` + `searchList/searchName/searchResult` |
| `explore` | `explore[]` / `category` | `getHome/getTab/getFilterList` | 无（可空） |
| `detail` | `comic.loadInfo(id)` | `getVideoDetail(video)` | 详情页 XPath（可空） |
| `chapters` | `comic.loadChapters(id)` | `getVideoEpisodes(video)` | `chapterRoads/chapterResult` |
| `pages` | `comic.loadEpisodes(chapter)` | — | 播放页 XPath / webview |
| `content` | —（小说源需自研） | — | — |
| `play` | — | `getVideoPlayUrl(video, episode)` | `useNativePlayer` / webview 嗅探 |

## 附录 B：参考资料

- Venera：<https://github.com/venera-app/venera>（README、`doc/comic_source.md`、`doc/js_api.md`、`alt_store.json`）
- Venera 源索引：<https://raw.githubusercontent.com/handahao666-boop/venera_comic_source/main/index.json>
- uzVideo：<https://github.com/YYDS678/uzVideo>；扩展规范：<https://github.com/YYDS678/uzVideo-extensions>
- Kazumi：<https://github.com/Predidit/Kazumi>；规则仓库：<https://github.com/Predidit/KazumiRules>
- uzVideo 视频源（type 101）：<https://github.com/YYDS678/uzVideo-extensions/tree/main/vod>
- 同类「只存地址、不存内容」的聚合索引（2026-10-21 调研）：
  - Legado（阅读）书源合集：<https://github.com/aoaostar/legado>（聚合 3900+ 条书源 JSON）
  - Legado 书源（自建）：<https://github.com/jiwangyihao/source-j-legado>
  - Mihon / Tachiyomi 漫画扩展索引：<https://github.com/keiyoushi/extensions>
  - YuanHsing/freed（影视 / 阅读 / 漫画源与接口汇总）：<https://github.com/YuanHsing/freed>
  - MoonTV（CMS 采集源聚合站）：<https://github.com/senshinya/MoonTV>（已迁
    <https://github.com/MoonTechLab/LunaTV>）
  - LibreTV（CMS V10 聚合）：<https://github.com/bestZwei/LibreTV>（现为
    <https://github.com/LibreSpark/LibreTV>）
  - 说明：MoonTV / LibreTV 都是「空壳」项目，**不内置采集站**，源列表由部署者自行提供；
    公开 TVBox 配置（如 <https://github.com/gaotianliuyun/gao>、
    <https://github.com/liu673cn/box>）里的 `api.php/provide/vod` 站点才是实际候选。
  - 本机直连 `raw.githubusercontent.com` 会被重置，读索引 / 脚本可用
    `https://ghproxy.net/<raw-url>` 镜像（GitHub API 不走该镜像）。
  - 说明：Legado / Tachiyomi / TVBox 的规则结构互不兼容（JSON / Kotlin / JSON 站点表），
    不能直接导入 RSP；可借鉴的是「索引只记上游地址」的组织方式。
- Apple App Review Guidelines：<https://developer.apple.com/app-store/review/guidelines/>（2.5.2 / 4.7 / 5.2 / 3.1）
- Apple Developer Program 费用：<https://developer.apple.com/programs/whats-included/>（99 USD/年）
- App Store Connect 协议 / 税务 / 银行：<https://developer.apple.com/help/account/agreements/>（免费 App 只需 Free Apps Agreement）
- 中国大陆区 App 备案：<https://developer.apple.com/cn/support/icp/>（以 Apple 与工信部最新要求为准）
- LiveContainer（容器 / JIT-Less 签名）：<https://github.com/LiveContainer/LiveContainer>
- LiveContainer 安装文档：<https://livecontainer.github.io/docs/installation>
- Impactor（Windows / macOS / Linux 签名安装）：<https://github.com/claration/impactor>
- SideStore（设备端刷新）：<https://sidestore.io/>；文档：<https://docs.sidestore.io/>
- SideStore 自定义 Anisette 服务器：<https://docs.sidestore.io/docs/advanced/anisette>
- anisette-v3-server（自建）：<https://github.com/Dadoum/anisette-v3-server>
- SideStore 常见问题（pairing / AFC）：<https://docs.sidestore.io/docs/troubleshooting/common-issues>
- Sideloadly（单 IPA 签名）：<https://sideloadly.io/>
- AltStore / AltServer（Windows）：<https://altstore.io/>
- JitterbugPair（Windows 配对文件）：<https://github.com/osy/Jitterbug/releases>
- Google Play Developer Program Policy：<https://support.google.com/googleplay/android-developer/answer/16933379>（知识产权）
- Google Play Device and Network Abuse：<https://support.google.com/googleplay/android-developer/answer/16559646>
- Microsoft Store Policies：<https://learn.microsoft.com/windows/apps/publish/store-policies>
- Lunac 插件规范：`<exe 根>\Modules\README.md`（清单 / 权限 / 窗口 / sidecar / 单文件 ESM）

## 附录 C：App Store Lite 提交检查表

- [ ] `store` 构建中无 JS 执行、无来源市场、无远程规则导入
- [ ] 内置来源均可出示授权；无 NSFW
- [ ] 隐私清单 / 数据收集声明 / 第三方 SDK 清单
- [ ] 年龄分级与（如有）年龄门
- [ ] 举报 / 屏蔽 / 客服入口（如有 UGC）
- [ ] 审核备注说明「阅读器 / 播放器不提供内容」
- [ ] 截图与图标不含第三方版权素材
- [ ] 可复现的审核演示账号 / 演示来源（如需要）

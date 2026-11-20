---
name: resource-hub
description: 给「阅读」插件编写、校验、安装漫画 / 小说 / 番剧 / 电视剧来源规则（RSP 声明式规则），或把用户想看的作品直接交给阅读窗搜索。用户要新增来源、把某个站点做成规则、转换 Kazumi 规则、排查来源失败时使用。
---

# 资源库来源规则

把「用户给一个站点」变成一条可运行的 RSP 声明式来源，或把「用户想看某个作品」交给阅读窗搜索。

## 何时用哪个工具

| 工具 | 入参 | 性质 | 用在哪一步 |
|---|---|---|---|
| `ResourceHubSearch` | `query?` / `kind?`（comic·novel·anime·tv）/ `limit?` | 只读、本地 | 先查重：本机是不是已经有同 id / 同站点的来源 |
| `SourceValidate` | `file_path` | 只读、不联网 | 写完规则先做形状校验（清单字段 + 规则段 + 选择器） |
| `SourceDiagnose` | `file_path`、`query?` | 会向站点发一次真实请求 | **要审批**。只看「请求通不通 + 返回的是不是 HTML/JSON」，不跑选择器 |
| `SourceInstall` | `file_path`、`force?` | 写类 | **要审批**。装进宿主决定的位置；同 id 已存在必须显式 `force=true`（先问用户） |
| `ResourceHubOpen` | `query`（1..200 字）、`kind?` | 纯数据 | 不联网、不开窗：只产出一张「在阅读里搜索」卡，用户点卡才开窗并自动搜 |

- 只读档 / 计划相位 / 无人值守下 `SourceInstall` 会被拒 —— 不要绕路自己写文件。
- `SourceValidate` 只做**形状**校验；RSP 语义校验在插件加载时由 TS SDK 再做一次，坏条目会被丢弃。

## 来源文件形状（唯一入口形状）

```json
{ "manifest": { ... }, "rule": { ... } }
```

`manifest` 必填：`rsp`（1）、`id`（`[a-z0-9._-]`，≤48 字符，不以 `.` 开头）、`name`、
`version`、`kind`（至少一项，取 `comic` / `novel` / `anime` / `tv`）、`engine`（当前只支持
`declarative`）、`entry`（来源包内相对 `.json` 路径，声明式统一写 `rule.json` 即可）、
`capabilities`（`search` / `explore` / `detail` / `chapters` / `pages` / `content` / `play` / `auth`）。
可选：`baseUrl`（http/https，相对地址靠它解析）、`homepage`、`license`、`headers`、
`rateLimit`（`{rps, burst}`）。

`rule` 的段：`search` / `explore` / `chapters` / `pages` / `content` / `play`，至少一段。每段：

```json
{
  "request": { "method": "GET", "url": "https://site/search?q=@keyword&page=@page",
               "query": { "k": "v" }, "headers": { "Referer": "https://site/" }, "body": "" },
  "list":    { "css": ".book-item" },
  "fields":  { "title": { "selector": { "css": ".name" } },
               "url":   { "selector": { "css": "a", "attr": "href" } } }
}
```

- 翻页**只靠 `@page` 占位符**（执行器把页码塞进 `url` / `query` / `body`）；
  `paging` 字段目前只是清单层声明、执行器不读它，不要依赖它。
- `list` 是 `Selector`：`css` / `xpath` / `jsonPath` **至少写一个**。
- **接口返回 JSON 时必须用 `list.jsonPath`** —— 执行器只有看到 `jsonPath` 才按 JSON 解析，
  否则会把响应体当 HTML。JSON 来源的字段 selector 也一路写 `jsonPath`（如 `$.data.list[*]`、`$.name`）。
- `fields` 里每个字段是 `{ selector, attr?, text?, regex?, template? }`：不写 `attr` 取文本；
  `regex` 取第一个捕获组；`template` 把结果套进模板，`@value` 代表原值。
- 占位符直接写进 `request.url` / `query` / `body`：`@keyword` `@page` `@section` `@workId`
  `@workUrl` `@chapterId` `@chapterUrl` `@sourceId`（未知占位符原样保留，便于排错）。
- `chapters` 可以加 `sub: { list, fields }` 表达「线路 → 剧集」两层（先取线路，再在线路节点内取剧集）。
- **字段名必须对得上执行器**，否则结果为空或标题变成 id：
  - `search` / `explore`：`id` 或 `url`（作品引用）、`title`（必填）、`author`、`cover`、`description`、`tags`
  - `chapters`：`id` 或 `url`、`title`
  - `pages`：`url` 或 `src`
  - `content`：`content` 或 `text`；另用顶层 `contentFormat` 说明 `text` / `html` / `epub`
  - `play`：`url` 或 `playUrl`（`.m3u8` 自动按 HLS）
- 相对地址一律按 `manifest.baseUrl` 解析；防盗链站点把 `Referer` / `User-Agent` 写进 `manifest.headers`。
- 两个特例：`cms`（Apple CMS v10 采集接口，只写 `api` 地址）与 `sniff`（播放地址靠 WebView 嗅探，
  宿主需声明 `permissions: ["webview.sniff"]`）。二选一即可，不要和通用 `play` 混写。

## 工作流（按顺序，不要跳）

1. 先问清站点与内容类型，**不要猜 URL**；用 `ResourceHubSearch` 查重（同 id / 同站点）。
2. 在工作区写一个 `.json` 文件，形状就是上面的 `{ manifest, rule }`。
3. `SourceValidate` 校验：`errors` 必须清零；`warnings` 原样报告给用户，不要静默忽略。
4. `SourceDiagnose`（会弹审批）发一次真实请求，看 HTTP 状态、Content-Type、体量、
   像 HTML 还是 JSON，以及 `hint`。这一步只证明「能通 + 形态对」。
5. 用户确认后 `SourceInstall`（会弹审批）安装。装完告诉用户去「阅读」插件里试搜一次
   —— 插件会用 TS SDK 再校验一遍。
6. 失败时按错误码读：`network`（连不上 / 超时）、`parse`（响应不是预期 JSON）、
   `auth`（401 要登录）、`blocked`（403 被拒）、`unsupported`（规则没声明该能力）、`unknown`。
   常见成因：列表选择器命中 0 条（先确认 diagnose 的 body 真是 HTML/JSON）、
   分页参数名写错、封面是相对地址但没配 `baseUrl`、站点要 Referer。

## 对话入口（只搜作品，不造来源）

用户说「帮我找 / 我想看某某漫画（小说 / 番剧 / 剧）」且不需要新来源时，调
`ResourceHubOpen`（`query` 必填，`kind` 可选）。它**不联网、不开窗**，只返回一条卡片载荷；
用户点卡片才会打开阅读窗并自动搜索。不要用它替代 `SourceValidate` / `SourceInstall`。

## Kazumi 规则转换

只做**格式转换**：读 `searchURL` / `searchList` / `searchName` / `searchResult` /
`chapterRoads` / `chapterResult` / `searchApiConfig`，输出 RSP 声明式规则。
不 import、不内嵌上游代码，不假装支持 `useWebview` / `muliSources` / `adBlocker`
（这些写进 `warnings`）。

## 硬边界

- 不复制上游脚本正文（Venera GPL-3.0、uzVideo 未见 LICENSE）；`license` / `attribution` 必须保留。
- 不把第三方规则打进发行物；规则由用户自行导入。
- 不上传用户的 Cookie / 凭据，不把登录态写进规则。
- 优先声明式；JS 来源必须走宿主 QuickJS 沙箱（`engine.eval`），不跑未校验的远程 JS。
- 不自己拼 `ModuleData\resource-hub\sources`，安装只走 `SourceInstall`（目录由宿主决定）。

## 输出要求

- 新增 / 修改的文件路径。
- `SourceValidate` 的完整输出（含 warnings）；`SourceDiagnose` 的关键结论（状态码 / 形态 / hint）。
- 规则只覆盖部分能力时，明确列出「未覆盖」的字段与能力。

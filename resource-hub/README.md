# Resource Hub（资源库）

漫画 / 小说 / 番剧 / 电视剧聚合平台的共享内核与 Lunac 插件外壳。

- **Windows**：Lunac 拓展插件（`apps/lunac-plugin`）。
- **iOS / Android**：独立应用，公开仓库 [LythrumMoon/lunac-reader-mobile](https://github.com/LythrumMoon/lunac-reader-mobile)，复用 `packages/*`，不依赖 Lunac 宿主。
- 设计说明：[`../docs/resource-hub-design.md`](../docs/resource-hub-design.md)。

## 当前进度

- `packages/rh-source-sdk`：RSP v1 类型、清单校验、规则格式转换（**只做格式转换，不依赖第三方运行时**）、宿主 API v3 契约与 mock、声明式规则求值 / 执行器；EPUB 解析、JS 源运行时、Venera `ComicSource` 与 uzVideo type 101 兼容层。
- `packages/rh-reader`：阅读器状态机与上下框 UI（35-30-35 分区、自动隐藏、锁定、自动播放、速度、进度条、跨章按需加载、书签）；iPad HD 断点与安全区适配。
- `apps/lunac-plugin`：可加载的插件外壳（本地书库 / 搜索 / 来源导入 / 阅读器 / 视频播放器），单文件 ESM 产物。
- 宿主 API v3 的 Rust 实现已接入（`net.fetch` / `media.proxy` / `storage.plugin` / `engine.eval`）；
  来源网络层已有缓存、限速与 `AbortSignal`。
- 搜索与总览按来源 `Paged.next` 真分页（滚到底部联网取下一页并按作品去重追加）。
- 尚未接入：移动端外壳；原生解码器（MKV 按直链试播，HEVC 等编码提示换线路 / 复制直链）。
  下载 / 离线缓存已按用户裁定不做。

## 构建

```bash
cd resource-hub
npm install
npm run build
```

产物：

- SDK：`packages/rh-source-sdk/dist/`
- 插件：`apps/lunac-plugin/dist/index.js` + `lunac-plugin.json`

## 安装到 Lunac（Windows）

**正式路径（2026-11-01 起）= 主仓库的插件打包脚本**（从仓库根执行）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1 -Plugin resource-hub
```

它会在 `resource-hub` 里跑 `npm run build`，然后把 `apps/lunac-plugin/dist/` 同时铺到三处：

- `app\plugin-dist\resource-hub\` —— 统一构建产物（后续打 zip 用）；
- `release\ext-plugins\resource-hub\` —— 安装包暂存（NSIS 的「Reading (resource hub)」勾选项）；
- `app\src-tauri\target\debug\Modules\resource-hub\` —— dev 实例真正读的那一份。

然后在「设置 → 插件 → 重新扫描」中刷新（或重启 Lunac）。

> ⚠️ **改了源码必须重跑上面那条脚本**（不要只跑本目录的 `npm run build`）：宿主读的是盘上那份
> `index.js`，不是 `src/`。漏了这一步的表现是「代码明明改了，dev 里行为没变、日志也没有报错」。
> 确认跑的是哪一版：设置 → 插件里「阅读」显示的版本 = `public/lunac-plugin.json` 的 `version`
> （改了播放器等行为就把它 +1；该文件是**版本号唯一真相源**）。
>
> 只想手工铺一份 dev（不进 ext-plugins / zip）时才用：
>
> ```powershell
> Copy-Item -Path "resource-hub\apps\lunac-plugin\dist\*" -Destination "app\src-tauri\target\debug\Modules\resource-hub\" -Recurse -Force
> ```

> 插件内置演示数据；导入公开规则仓库后可启用声明式来源。宿主 API v3
> （`net.fetch` / `media.proxy` / `storage.plugin` / `engine.eval`）已落地。

## 测试

```bash
npm test
```

本地 UI 验证：`apps/lunac-plugin/test/harness.html`（在插件目录起 `npx vite --port 8123`，
访问 `http://localhost:8123/test/harness.html`）。这是开发用页面，不随插件包分发。
加 `?fixture=1` 会导入本地声明式来源夹具，可验证「搜索 → 详情 → 章节 → 阅读」链路；
`?fixture=video|epub|js|venera|uzvideo` 分别验证视频、EPUB、JS 源、Venera 与 uzVideo 兼容层。

## 规则仓库

```bash
npm run registry:build          # registry/upstream/*.json → registry/index.json
npm run source:validate -- x.json
```

- 只发布索引与元数据，不复制规则 / 脚本正文；插件安装条目时再按 `sourceUrl` 读取。
- 当前已生成 `registry/index.json`：59 条（含自建演示 1 条）。
  自建 `manual` 条目内嵌规则，可在插件「来源」页导入后一键安装；详见 [`registry/README.md`](registry/README.md)。
- 公开规则仓库：[LythrumMoon/lunac-rules](https://github.com/LythrumMoon/lunac-rules)。
  插件导入页默认使用 `https://cdn.jsdelivr.net/gh/LythrumMoon/lunac-rules@main/index.json`，
  raw 备选 `https://raw.githubusercontent.com/LythrumMoon/lunac-rules/main/index.json`。
- 详见 [`registry/README.md`](registry/README.md) 与 `docs/resource-hub-design.md` §5.8。

## AI 侧

- 技能正文随 Lunac 一起发布：`agent-templates/skills/resource-hub/SKILL.md`
  （装完即 `<exe 根>\skills\resource-hub\SKILL.md`；这里不再放第二份，避免两处漂移）。
- `npm run source:validate -- <file>` 是模型生成规则后的必经校验。

## 来源与许可

- 真实规则由用户自行导入；来源许可与合规由用户与规则作者承担。
- 本项目不内嵌、不分发第三方规则正文，也不提供内容。

## 许可

MIT。个人项目、非商用、无广告、无遥测、不提供内容。

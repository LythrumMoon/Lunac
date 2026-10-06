// ── 磁盘插件（Modules\<id>\）的独立打包配置（2026-09-28）─────────────
//
// 用途：把**内置插件**打包成「可独立安装的磁盘插件」，投进 `<exe 根>\Modules\<id>\`，
// 由插件市场按需分发（不再随主 bundle 一起装）。产物：`app/plugin-dist/<id>/index.js`
// —— 一份**自包含的 ESM**（tauri API 等依赖全部内联），配合 `scripts/build-plugins.ps1`
// 生成的 `lunac-plugin.json` 与 zip 就是完整的插件包。
//
// **为什么用 alias 而不是改插件源码**：插件源码里写的是 `import { t } from "../../i18n.js"`
// ——那是「内置插件」的写法。独立打包时，这份 i18n 会是**另一份未初始化的副本**（语言状态
// 不在里面）⇒ 界面会退回显示 key。所以这里把 `../../i18n.js` **别名到 `plugins/host.ts`**：
// 它导出的 `t` 走宿主桥 `globalThis.__lunac_host`（见 host.ts 头注释）。
// 好处是**插件源码一个字都不用改**，内置构建与磁盘构建共用同一份文件。
//
// 与主构建的关系：`vite.config.ts` 负责 `src/index.html` + `src/plugin.html`（前端本体），
// 本文件只负责插件包，**两者互不影响**（`npm run build` 不会跑到这里，见 build-plugins.ps1）。
//
// 注意：**只有自包含、不依赖宿主私有状态的插件才适合搬出去**。当前 `layout: "takeover"`
// 这类特权已由 `Plugin.permissions`（`"layout.takeover"`，OCR 用）声明式开放给磁盘插件 ——
// 宿主侧判定见 main.ts 的 `isTakeoverPlugin()`。
//
// ── 为什么**一次只打一个入口**（2026-09-29 修）─────────────────────
// 四个入口放进**一次**构建时，Rollup 会把多入口共享的模块（host 桥 / @tauri-apps/api）
// 提到 `plugin-dist/<chunk 名>/chunk-*.js` 这种**跨插件的公共 chunk**里，于是
// `plugin-dist/ocr/index.js` 头部长成：
//     import { t } from "../host/chunk-D8dymyFk.js";
// 而插件包（以及 zip）只搬走**它自己那个目录** ⇒ 那份 chunk 不在包里，import 直接 404，
// 插件打开即失败（实测：装上 OCR 后面板空白、控制台报模块加载失败）。
// 一次构建只放**一个**入口就没有跨入口共享，产物必然是单文件、自包含。
//
// Vite 的 CLI **不支持一个配置文件导出多份配置**（那是 Rollup 的用法，实测报
// 「config must export or return an object」），所以由 `scripts/build-plugins.ps1`
// 逐个入口各起一次 `vite build`，用环境变量 `LUNAC_PLUGIN=<id>` 指定这一次打谁。

import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const p = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));

/** 要打包成磁盘插件的入口。键 = 插件 id（同时是 `Modules\<id>\` 的目录名）。 */
const pluginEntries: Record<string, string> = {
  music: p("./src/plugins/builtin/music.ts"),
  "clipboard-history": p("./src/plugins/builtin/clipboard-history.ts"),
  ocr: p("./src/plugins/builtin/ocr.ts"),
  convert: p("./src/plugins/builtin/convert.ts"),
  proxy: p("./src/plugins/builtin/proxy.ts"),
  pet: p("./src/plugins/builtin/pet.ts"),
};

/** 本次打哪一个（由 build-plugins.ps1 逐个设置）。 */
const only = (process.env.LUNAC_PLUGIN ?? "").trim();
if (!only || !(only in pluginEntries)) {
  throw new Error(
    `请设置环境变量 LUNAC_PLUGIN=<id> 指定要打包的插件（可选：${Object.keys(pluginEntries).join(" / ")}）；` +
      `整包构建请用 scripts/build-plugins.ps1（它会逐个入口各调一次）。`,
  );
}
// ── 附加入口（可选，2026-09-30 为桌宠的 Live2D 引擎加的）─────────────────
// `LUNAC_PLUGIN_EXTRA_OUT` = 产物在插件目录内的相对路径（如 `engine/live2d-engine.js`）
// `LUNAC_PLUGIN_EXTRA_SRC` = 源码路径（相对 app/，如 `src/plugins/builtin/live2d-engine.ts`）
// 由 `scripts/build-plugins.ps1` 按插件的 `extraEntries` 逐个**再起一次**本配置。
//
// **为什么是「同一插件的第二次构建」而不是多入口**：插件包里的每个 js 都必须是自包含单文件
// —— 多入口会让 Rollup 把共享模块提成公共 chunk，而宿主是用 `import(convertFileSrc(...))`
// 装载的，asset 协议把整条绝对路径 percent-encode 进 URL 最后一段，**相对 specifier 必然
// 落到协议根**⇒ 那个 chunk 404（见下方 `inlineDynamicImports` 那段）。而有些模块**必须晚于
// 某个运行时全局脚本才敢求值**（`pixi-live2d-display` 在模块求值时就检查
// `window.Live2DCubismCore`），`inlineDynamicImports` 又会把它提前到顶层 ⇒ 只能拆成独立文件、
// 由插件运行时按**绝对 URL** 动态 import（见 `src/plugins/builtin/live2d.ts` 的文件头）。
const extraOut = (process.env.LUNAC_PLUGIN_EXTRA_OUT ?? "").trim();
const extraSrc = (process.env.LUNAC_PLUGIN_EXTRA_SRC ?? "").trim();
if (extraOut && (!extraSrc || extraOut.includes("..") || extraSrc.includes(".."))) {
  throw new Error(`附加入口参数不合法（out=${extraOut} / src=${extraSrc}）—— 两者都不许含 ".."`);
}
const slash = extraOut.lastIndexOf("/");
const outSub = slash >= 0 ? extraOut.slice(0, slash) : "";
const outName = slash >= 0 ? extraOut.slice(slash + 1) : extraOut;

const entry = extraOut ? p(`./${extraSrc.replace(/^\.\//, "")}`) : pluginEntries[only];

export default defineConfig({
  root: "src",
  // 库模式不需要 public/ 里的静态资源，也不该把它们复制进插件包
  publicDir: false,
  build: {
    // 一个插件一个目录（与清单的 entry 一致），只清自己那一份；
    // 附加入口落在它自己的子目录里（`emptyOutDir` 也只清那一层，不会碰主产物）
    outDir: outSub ? `../plugin-dist/${only}/${outSub}` : `../plugin-dist/${only}`,
    emptyOutDir: true,
    // 源码里没有 `.ts` 之外的资源，但显式关掉 sourcemap 能让包小一半
    sourcemap: false,
    minify: "esbuild",
    lib: {
      entry,
      formats: ["es"],
    },
    rollupOptions: {
      output: {
        // 单入口 ⇒ 产物就是一个文件。主入口叫 index.js（清单的 entry 默认值），
        // 附加入口用它自己在插件目录内的文件名。
        entryFileNames: extraOut ? outName : "index.js",
        // **产物必须是自包含单文件**：宿主用 `import(convertFileSrc(<插件目录>/index.js))`
        // 装载，而 asset 协议把**整条绝对路径** percent-encode 进 URL 最后一段 ⇒ 打包器
        // 生成的任何相对 specifier（`./chunk-xxx.js`）都会落到协议**根**、不是插件目录，
        // 于是那个文件 404、插件打开即失败（文件头那次「多入口拆出公共 chunk」就是这个坑）。
        // 这条把「万一将来真出现可分析的动态 import」也一并按死。
        inlineDynamicImports: true,
      },
    },
  },
  resolve: {
    alias: [
      // 见文件头注释：把宿主的 i18n 换成「读宿主桥」的实现
      { find: /^(?:\.\.\/)+i18n\.js$/, replacement: p("./src/plugins/host.ts") },
    ],
  },
});

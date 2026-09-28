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
// 这类特权（OCR 会接管整个窗口）宿主尚未开放给磁盘插件，所以 ocr 暂不在下面这张表里。

import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const p = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));

/** 要打包成磁盘插件的入口。键 = 插件 id（同时是 `Modules\<id>\` 的目录名）。 */
const pluginEntries: Record<string, string> = {
  music: p("./src/plugins/builtin/music.ts"),
};

export default defineConfig({
  root: "src",
  // 库模式不需要 public/ 里的静态资源，也不该把它们复制进插件包
  publicDir: false,
  build: {
    outDir: "../plugin-dist",
    emptyOutDir: true,
    // 源码里没有 `.ts` 之外的资源，但显式关掉 sourcemap 能让包小一半
    sourcemap: false,
    minify: "esbuild",
    lib: {
      entry: pluginEntries,
      formats: ["es"],
    },
    rollupOptions: {
      output: {
        // 一个插件一个目录：plugin-dist/<id>/index.js（与清单的 entry 一致）
        entryFileNames: "[name]/index.js",
        // 万一将来某个插件用了动态 import：chunk 也落在**它自己的目录**里。
        // 插件包必须是「一个目录搬走就能用」的形态，chunk 落到 plugin-dist 根下就散架了。
        chunkFileNames: "[name]/chunk-[hash].js",
        assetFileNames: "[name]/[name]-[hash][extname]",
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

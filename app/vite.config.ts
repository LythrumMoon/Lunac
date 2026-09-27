import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

export default defineConfig({
  root: "src",
  base: "./",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    // 多页入口（2026-09-27）：插件悬浮窗是**独立窗口**，由宿主
    // `plugin_window::open_plugin_window` 加载 `plugin.html`。
    // 不加这一项，`dist/` 里只会有 index.html ⇒ 打包版打开悬浮窗直接白屏
    // （dev 下 Vite 按需编译，反而看不出问题 —— 所以这条必须留在构建配置里）。
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./src/index.html", import.meta.url)),
        plugin: fileURLToPath(new URL("./src/plugin.html", import.meta.url)),
      },
    },
  },
  // Prevent vite from obscuring Rust errors
  clearScreen: false,
  // Tauri expects a fixed port for dev mode
  server: {
    port: 5173,
    strictPort: true,
  },
  // Env variables starting with TAURI_ will be exposed
  envPrefix: ["VITE_", "TAURI_"],
});

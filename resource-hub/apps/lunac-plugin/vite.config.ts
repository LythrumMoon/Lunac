import { defineConfig } from "vite";

// 插件入口必须是**单文件 ESM**（Lunac 硬约束）：
// 不产生公共 chunk、不 import 任何网络地址，SDK 一起内联进 index.js。
export default defineConfig({
  publicDir: "public",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2020",
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: () => "index.js",
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
    },
  },
});

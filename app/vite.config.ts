import { defineConfig } from "vite";

export default defineConfig({
  root: "src",
  base: "./",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
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

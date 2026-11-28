import { defineConfig } from "vite";
// @ts-expect-error process is provided by Vite's node runtime; no @types/node in this app.
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;
const uiPort = Number(process.env.LOOM_UI_PORT) || 43127;
const enginePort = Number(process.env.LOOM_ENGINE_PORT) || 43128;

export default defineConfig(() => ({
  clearScreen: false,
  server: {
    port: uiPort,
    strictPort: true,
    host: host || "127.0.0.1",
    proxy: {
      "/api": {
        target: `http://127.0.0.1:${enginePort}`,
        changeOrigin: true,
      },
    },
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"],
    },
  },
}));

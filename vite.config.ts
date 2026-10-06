import { defineConfig } from "vite";
// @ts-expect-error process is provided by Vite's node runtime; no @types/node in this app.
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(() => ({
  clearScreen: false,
  server: {
    port: 43127,
    strictPort: true,
    host: host || "127.0.0.1",
    proxy: {
      "/api": {
        target: "http://127.0.0.1:43128",
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

import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

const host = process.env.TAURI_DEV_HOST;

// 版本真源：Tauri 配置（与安装包、Cargo.toml、frontend/package.json 保持同版本）
const tauriConf = JSON.parse(
  fs.readFileSync(
    fileURLToPath(new URL("../src-tauri/tauri.conf.json", import.meta.url)),
    "utf-8",
  ),
) as { version: string };

// https://vite.dev/config/
// https://v2.tauri.app/reference/config/
export default defineConfig(async () => ({
  plugins: [vue()],

  define: {
    __APP_VERSION__: JSON.stringify(tauriConf.version),
  },

  // Tauri expects a fixed port, fail if that port is not available
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // Tell vite to ignore watching src-tauri
      ignored: ["**/src-tauri/**"],
      usePolling: true,
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
}));

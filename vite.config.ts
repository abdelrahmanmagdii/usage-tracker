import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

const host = (globalThis as { process?: { env?: Record<string, string | undefined> } })
  .process?.env?.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  test: {
    environment: "jsdom",
    // tools/ holds the node:test suite for the Tibo Watch scraper (node --test).
    exclude: ["tools/**", "node_modules/**", "src-tauri/**"],
  },
});

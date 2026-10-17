/// <reference types="vitest/config" />

import path from "node:path";
import process from "node:process";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],

  // shadcn/ui components import through the `@/` alias (see tsconfig.json `paths`).
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
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
      // 3. tell Vite to ignore everything that is not frontend source: the Cargo `target/` tree
      // alone holds hundreds of thousands of files and stalls the watcher and dependency scan
      ignored: [
        "**/src-tauri/**",
        "**/target/**",
        "**/vendor/**",
        "**/crates/**",
        "**/docs/**",
        "**/tools/**",
        "**/dist/**",
      ],
    },
  },

  // Scan only the app entry for dependencies instead of every HTML file under the repo root
  optimizeDeps: { entries: ["index.html"] },

  // Vitest: component tests run in jsdom with Testing Library and the Tauri IPC mocks.
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));

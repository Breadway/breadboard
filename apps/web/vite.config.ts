import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Plain SPA build (no SSR). The Tauri shell consumes the same static output.
export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5280,
    // Dev proxy so the SPA can call the API on the same origin.
    proxy: {
      "/healthz": "http://127.0.0.1:7280",
      "/events": { target: "http://127.0.0.1:7280", ws: true },
      "/api": "http://127.0.0.1:7280",
      "/auth": "http://127.0.0.1:7280",
    },
  },
  build: {
    outDir: "dist",
    target: "es2022",
  },
});

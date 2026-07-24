import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

// tc-npc's Rust binary serves this UI and a WebSocket at the same origin
// (default 127.0.0.1:47950 — see crates/npc-server). The dev server proxies
// /ws, /api and /healthz there so `npm run dev` can hit a locally running
// binary without a CORS dance.
export default defineConfig({
  base: "./",
  plugins: [preact()],
  build: {
    outDir: "dist",
  },
  server: {
    proxy: {
      "/ws": {
        target: "ws://127.0.0.1:47950",
        ws: true,
      },
      "/api": {
        target: "http://127.0.0.1:47950",
      },
      "/healthz": {
        target: "http://127.0.0.1:47950",
      },
    },
  },
});

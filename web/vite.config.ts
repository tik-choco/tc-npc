/// <reference types="vitest/config" />
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
  test: {
    // Vitest's default excludes don't cover dotfolders like .claude/ — without
    // this, running from the repo root would also pick up and re-run every
    // test file inside any git worktree checked out under .claude/worktrees/
    // (e.g. background agents working in parallel), which is slow and makes
    // failures there look like failures here (see tc-town's vite.config.ts).
    exclude: ["**/node_modules/**", "**/dist/**", "**/.claude/**", "**/.git/**"],
  },
});

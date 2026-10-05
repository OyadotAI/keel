import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The budget is 400 KB gzipped (the plan's table); this warning is raw bytes, ~3x that.
// Tauri expects a fixed port and fails if it is taken, rather than drifting to another one the
// daemon's origin allowlist does not know (`APP_ORIGINS` in crates/keel/src/pair.rs).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", chunkSizeWarningLimit: 450 },
});

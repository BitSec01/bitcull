import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri serves the dev build from a fixed port and needs a stable host.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust sources are rebuilt by cargo, not vite.
      ignored: ["**/src-tauri/**", "**/test_photos/**"],
    },
  },
  build: {
    // Matches the webview shipped with Tauri v2.
    target: "es2021",
    sourcemap: false,
  },
});

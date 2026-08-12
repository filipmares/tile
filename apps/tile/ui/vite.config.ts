import { defineConfig } from "vite";

// Tauri expects a fixed dev port and relative asset paths in the built bundle.
export default defineConfig({
  base: "./",
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2021",
  },
});

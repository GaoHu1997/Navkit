import { defineConfig } from "vite";

// Tauri expects a fixed port and does not want vite to obscure rust errors.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5183,
    strictPort: true,
    // 开发期禁用缓存，避免 WebView2/浏览器拿到过期的 preview-bootstrap.js
    headers: { "Cache-Control": "no-store" },
    watch: { ignored: ["**/src-tauri/**", "**/reference_files/**", "**/reference_resources/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
    outDir: "dist",
    emptyOutDir: true,
  },
});

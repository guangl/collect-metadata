import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  root: "ui",
  base: "./",
  plugins: [react()],
  server: {
    host: "localhost",
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: ["safari15.4", "chrome105"],
    rolldownOptions: {
      output: {
        entryFileNames: "app.js",
        assetFileNames: (asset) => asset.names.some((name) => name.endsWith(".css")) ? "style.css" : "[name][extname]",
      },
    },
  },
});

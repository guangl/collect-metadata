import react from "@vitejs/plugin-react";
import autoprefixer from "autoprefixer";
import tailwindcss from "tailwindcss";
import { defineConfig } from "vitest/config";

export default defineConfig({
  root: "ui",
  base: "./",
  plugins: [react()],
  css: {
    postcss: {
      plugins: [tailwindcss(), autoprefixer()],
    },
  },
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
  test: {
    root: ".",
    include: ["ui/spec/**/*.test.{ts,tsx}"],
    environment: "jsdom",
    coverage: {
      provider: "v8",
      include: ["ui/src/**/*.{ts,tsx}"],
      reporter: ["text", "lcov"],
      thresholds: { lines: 85 },
    },
  },
});

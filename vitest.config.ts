import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
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

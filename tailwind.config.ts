import type { Config } from "tailwindcss";

export default {
  content: {
    relative: true,
    files: ["./ui/index.html", "./ui/src/**/*.{ts,tsx}"],
  },
  theme: {
    extend: {
      colors: {
        canvas: "rgb(var(--color-canvas) / <alpha-value>)",
        header: "rgb(var(--color-header) / <alpha-value>)",
        surface: "rgb(var(--color-surface) / <alpha-value>)",
        raised: "rgb(var(--color-raised) / <alpha-value>)",
        line: "rgb(var(--color-line) / <alpha-value>)",
        muted: "rgb(var(--color-muted) / <alpha-value>)",
        ink: "rgb(var(--color-ink) / <alpha-value>)",
        accent: "rgb(var(--color-accent) / <alpha-value>)",
        "accent-hover": "rgb(var(--color-accent-hover) / <alpha-value>)",
        "accent-surface": "rgb(var(--color-accent-surface) / <alpha-value>)",
        "on-accent": "rgb(var(--color-on-accent) / <alpha-value>)",
        success: "rgb(var(--color-success) / <alpha-value>)",
        warning: "rgb(var(--color-warning) / <alpha-value>)",
        danger: "rgb(var(--color-danger) / <alpha-value>)",
      },
      fontFamily: {
        sans: ['"PingFang SC"', '"Microsoft YaHei UI"', '"Segoe UI"', "system-ui", "sans-serif"],
        mono: ['"SF Mono"', "Consolas", '"Liberation Mono"', "monospace"],
      },
      spacing: {
        gutter: "clamp(1rem, 4vw, 2.75rem)",
      },
      maxWidth: {
        workspace: "77.5rem",
      },
      gridTemplateColumns: {
        posters: "repeat(auto-fill, minmax(min(100%, 7rem), 1fr))",
        "posters-wide": "repeat(auto-fill, minmax(min(100%, 9.5rem), 1fr))",
        library: "repeat(auto-fill, minmax(min(100%, 5.75rem), 1fr))",
        "library-wide": "repeat(auto-fill, minmax(min(100%, 8.25rem), 1fr))",
        credits: "repeat(auto-fill, minmax(min(100%, 4.875rem), 1fr))",
        "credits-wide": "repeat(auto-fill, minmax(min(100%, 6.125rem), 1fr))",
      },
    },
  },
} satisfies Config;

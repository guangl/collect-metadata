import { useLayoutEffect, useState } from "react";

export const themeChoices = [
  ["system", "跟随系统"],
  ["light", "浅色"],
  ["dark", "深色"],
] as const;
export type Theme = typeof themeChoices[number][0];
const key = "collect-metadata-theme";

function readTheme(): Theme {
  try {
    const saved = localStorage.getItem(key);
    if (saved === "light" || saved === "dark") return saved;
  } catch { /* The workspace can run with storage disabled. */ }
  return "system";
}

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(readTheme);
  useLayoutEffect(() => {
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    function apply() {
      document.documentElement.dataset.theme = theme === "system" ? (media?.matches ? "dark" : "light") : theme;
    }
    apply();
    if (theme === "system") media?.addEventListener("change", apply);
    try { localStorage.setItem(key, theme); } catch { /* Keep this session's choice. */ }
    return () => media?.removeEventListener("change", apply);
  }, [theme]);
  return [theme, setTheme] as const;
}

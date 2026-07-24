// Explicit light/dark theme, defaulting to LIGHT (we intentionally do not
// follow the OS `prefers-color-scheme` by default). The choice is applied as
// a `data-theme` attribute on <html> and persisted so it survives reloads.
// index.html runs the same read (localStorage['tc-npc:theme']) before first
// paint to avoid a flash of the wrong theme.
import { useEffect, useState } from "preact/hooks";

export type Theme = "light" | "dark";

const THEME_KEY = "tc-npc:theme";

function initialTheme(): Theme {
  try {
    return localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

export interface UseThemeResult {
  theme: Theme;
  toggleTheme(): void;
}

export function useTheme(): UseThemeResult {
  const [theme, setTheme] = useState<Theme>(() => initialTheme());

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem(THEME_KEY, theme);
    } catch {
      // Ignore storage failures (private browsing, etc).
    }
  }, [theme]);

  function toggleTheme() {
    setTheme((t) => (t === "light" ? "dark" : "light"));
  }

  return { theme, toggleTheme };
}

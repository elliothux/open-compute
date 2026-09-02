import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type ThemeMode = "light" | "dark" | "system";

interface ThemeContextValue {
  mode: ThemeMode;
  resolved: "light" | "dark";
  setMode: (mode: ThemeMode) => void;
  toggle: () => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);
const STORAGE_KEY = "open-compute-dashboard-theme";

function resolveTheme(mode: ThemeMode): "light" | "dark" {
  if (mode === "system") {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  return mode;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    return stored === "light" || stored === "dark" || stored === "system" ? stored : "system";
  });
  const resolved = useMemo(() => resolveTheme(mode), [mode]);

  useEffect(() => {
    document.documentElement.dataset.mode = resolved;
    localStorage.setItem(STORAGE_KEY, mode);
  }, [mode, resolved]);

  useEffect(() => {
    if (mode !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const listener = () => {
      document.documentElement.dataset.mode = resolveTheme("system");
    };
    media.addEventListener("change", listener);
    return () => media.removeEventListener("change", listener);
  }, [mode]);

  const value = useMemo<ThemeContextValue>(() => ({
    mode,
    resolved,
    setMode: setModeState,
    toggle: () => {
      setModeState(current => {
        const next = resolveTheme(current === "system" ? "system" : current);
        return next === "dark" ? "light" : "dark";
      });
    },
  }), [mode, resolved]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme() {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used within ThemeProvider");
  return context;
}

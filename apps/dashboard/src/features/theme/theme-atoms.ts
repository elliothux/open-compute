import { atom, useAtomValue, useSetAtom } from "jotai";

export type ThemeMode = "light" | "dark" | "system";

const STORAGE_KEY = "open-compute-dashboard-theme";

function storedTheme(): ThemeMode {
  if (typeof localStorage === "undefined") return "system";
  const value = localStorage.getItem(STORAGE_KEY);
  return value === "light" || value === "dark" || value === "system"
    ? value
    : "system";
}

export const themeModeAtom = atom<ThemeMode>(storedTheme());
export const systemThemeAtom = atom<"light" | "dark">("light");
export const resolvedThemeAtom = atom<"light" | "dark">((get) => {
  const mode = get(themeModeAtom);
  return mode === "system" ? get(systemThemeAtom) : mode;
});

const setThemeModeAtom = atom(null, (_get, set, mode: ThemeMode) => {
  if (typeof localStorage !== "undefined")
    localStorage.setItem(STORAGE_KEY, mode);
  set(themeModeAtom, mode);
});

const toggleThemeAtom = atom(null, (get, set) => {
  set(setThemeModeAtom, get(resolvedThemeAtom) === "dark" ? "light" : "dark");
});

export function useTheme() {
  return {
    mode: useAtomValue(themeModeAtom),
    resolved: useAtomValue(resolvedThemeAtom),
    setMode: useSetAtom(setThemeModeAtom),
    toggle: useSetAtom(toggleThemeAtom),
  };
}

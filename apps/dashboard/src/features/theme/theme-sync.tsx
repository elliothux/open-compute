import { useAtomValue, useSetAtom } from "jotai";
import { useEffect } from "react";
import {
  resolvedThemeAtom,
  systemThemeAtom,
  themeModeAtom,
} from "./theme-atoms";

export function ThemeSync() {
  const mode = useAtomValue(themeModeAtom);
  const resolved = useAtomValue(resolvedThemeAtom);
  const setSystemTheme = useSetAtom(systemThemeAtom);

  useEffect(() => {
    document.documentElement.dataset.mode = resolved;
  }, [resolved]);

  useEffect(() => {
    if (mode !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const synchronize = () => setSystemTheme(media.matches ? "dark" : "light");
    synchronize();
    media.addEventListener("change", synchronize);
    return () => media.removeEventListener("change", synchronize);
  }, [mode, setSystemTheme]);

  return null;
}

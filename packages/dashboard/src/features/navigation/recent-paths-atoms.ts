import { atom } from "jotai";

const STORAGE_KEY = "open-compute.operator.recent";
const MAX_RECENT_PATHS = 4;

function readRecentPaths(): string[] {
  if (typeof sessionStorage === "undefined") return [];
  try {
    const value: unknown = JSON.parse(
      sessionStorage.getItem(STORAGE_KEY) ?? "[]",
    );
    if (!Array.isArray(value)) throw new Error("invalid recent paths");
    return value
      .filter(
        (path): path is string =>
          typeof path === "string" && path.startsWith("/"),
      )
      .slice(0, MAX_RECENT_PATHS);
  } catch {
    sessionStorage.removeItem(STORAGE_KEY);
    return [];
  }
}

export const recentPathsAtom = atom(readRecentPaths());
export const recordRecentPathAtom = atom(null, (get, set, path: string) => {
  const next = [
    path,
    ...get(recentPathsAtom).filter((value) => value !== path),
  ].slice(0, MAX_RECENT_PATHS);
  if (typeof sessionStorage !== "undefined") {
    sessionStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  }
  set(recentPathsAtom, next);
});

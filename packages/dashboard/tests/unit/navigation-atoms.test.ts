import { describe, expect, test } from "bun:test";
import { createStore } from "jotai";
import {
  closeCommandPaletteAtom,
  commandPaletteOpenAtom,
  commandPaletteQueryAtom,
} from "../../src/features/navigation/command-palette-atoms";
import {
  recentPathsAtom,
  recordRecentPathAtom,
} from "../../src/features/navigation/recent-paths-atoms";

describe("navigation atoms", () => {
  test("close resets command state", () => {
    const store = createStore();
    store.set(commandPaletteOpenAtom, true);
    store.set(commandPaletteQueryAtom, "worker");
    store.set(closeCommandPaletteAtom);
    expect(store.get(commandPaletteOpenAtom)).toBeFalse();
    expect(store.get(commandPaletteQueryAtom)).toBe("");
  });

  test("recent paths are deduplicated and bounded across route remounts", () => {
    const store = createStore();
    for (const path of ["/workers", "/kv", "/d1", "/r2", "/workers"]) {
      store.set(recordRecentPathAtom, path);
    }
    expect(store.get(recentPathsAtom)).toEqual([
      "/workers",
      "/r2",
      "/d1",
      "/kv",
    ]);
  });
});

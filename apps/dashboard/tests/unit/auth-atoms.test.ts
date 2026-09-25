import { describe, expect, test } from "bun:test";
import { createStore } from "jotai";
import {
  authClientAtom,
  authInstanceIdAtom,
  authTokenAtom,
  clearAuthAtom,
  setAuthInstanceIdAtom,
  setAuthTokenAtom,
} from "../../src/features/auth/auth-atoms";

describe("auth atoms", () => {
  test("session actions keep one authority and logout resets all derived state", () => {
    const store = createStore();
    expect(store.get(authTokenAtom)).toBeNull();
    expect(store.get(authClientAtom)).toBeNull();

    store.set(setAuthTokenAtom, "session-token");
    store.set(setAuthInstanceIdAtom, "0123456789abcdef0123456789abcdef");
    expect(store.get(authTokenAtom)).toBe("session-token");
    expect(store.get(authInstanceIdAtom)).toBe(
      "0123456789abcdef0123456789abcdef",
    );
    store.set(clearAuthAtom);
    expect(store.get(authTokenAtom)).toBeNull();
    expect(store.get(authInstanceIdAtom)).toBeNull();
    expect(store.get(authClientAtom)).toBeNull();
  });
});

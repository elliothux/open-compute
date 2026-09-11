import { describe, expect, test } from "bun:test";
import { createStore } from "jotai";
import {
  authAccountIdAtom,
  authClientAtom,
  authTokenAtom,
  clearAuthAtom,
  setAuthAccountIdAtom,
  setAuthTokenAtom,
} from "../../src/features/auth/auth-atoms";

describe("auth atoms", () => {
  test("session actions keep one authority and logout resets all derived state", () => {
    const store = createStore();
    expect(store.get(authTokenAtom)).toBeNull();
    expect(store.get(authClientAtom)).toBeNull();

    store.set(setAuthTokenAtom, "session-token");
    store.set(setAuthAccountIdAtom, "account-id");
    expect(store.get(authTokenAtom)).toBe("session-token");
    expect(store.get(authAccountIdAtom)).toBe("account-id");
    store.set(clearAuthAtom);
    expect(store.get(authTokenAtom)).toBeNull();
    expect(store.get(authAccountIdAtom)).toBeNull();
    expect(store.get(authClientAtom)).toBeNull();
  });
});

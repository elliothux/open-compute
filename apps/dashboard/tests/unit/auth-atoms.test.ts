import { afterEach, describe, expect, test } from "bun:test";
import { createStore } from "jotai";
import {
  authClientAtom,
  authInstanceIdAtom,
  authTokenAtom,
  clearAuthAtom,
  setAuthInstanceIdAtom,
  setAuthTokenAtom,
  signOutAtom,
} from "../../src/features/auth/auth-atoms";

const realFetch = globalThis.fetch;

describe("auth atoms", () => {
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  test("sign out revokes the server session before clearing local state", async () => {
    const calls: { url: string; init: RequestInit | undefined }[] = [];
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      calls.push({ url, init });
      return new Response(null, { status: 204 });
    }) as typeof fetch;
    const store = createStore();
    store.set(setAuthTokenAtom, "session-token");
    await store.set(signOutAtom);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.url).toBe("/operator/session");
    expect(calls[0]?.init?.method).toBe("DELETE");
    expect(calls[0]?.init?.headers).toEqual({
      authorization: "Bearer session-token",
    });
    expect(store.get(authTokenAtom)).toBeNull();
  });

  test("sign out clears local state when revocation fails", async () => {
    globalThis.fetch = (async () => {
      throw new Error("offline");
    }) as unknown as typeof fetch;
    const store = createStore();
    store.set(setAuthTokenAtom, "session-token");
    await store.set(signOutAtom);
    expect(store.get(authTokenAtom)).toBeNull();
    expect(store.get(authClientAtom)).toBeNull();
  });

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

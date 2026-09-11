import { describe, expect, test } from "bun:test";
import { createStore } from "jotai";
import {
  failLiveTailAtom,
  liveTailAtom,
  prependLiveTailRowAtom,
  selectLiveTailWorkerAtom,
  setLiveTailEnabledAtom,
  setLiveTailStatusAtom,
} from "../../src/features/observability/live-tail-atoms";

describe("live tail atoms", () => {
  test("worker changes reset state while callbacks share one bounded stream", () => {
    const store = createStore();
    store.set(selectLiveTailWorkerAtom, "worker-a");
    store.set(setLiveTailEnabledAtom, true);
    store.set(setLiveTailStatusAtom, "live");
    store.set(prependLiveTailRowAtom, {
      id: "event-a",
      timestamp: "2026-09-08T00:00:00.000Z",
      level: "info",
      source: "test",
    });
    expect(store.get(liveTailAtom).rows).toHaveLength(1);

    store.set(failLiveTailAtom, "connection failed");
    expect(store.get(liveTailAtom)).toMatchObject({
      enabled: false,
      status: "error",
    });
    store.set(selectLiveTailWorkerAtom, "worker-b");
    expect(store.get(liveTailAtom)).toEqual({
      workerId: "worker-b",
      enabled: false,
      status: "idle",
      error: null,
      rows: [],
    });
  });
});

import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { assertSnapshot } = await importRuntime("loader/snapshot.ts");

function snapshot(props) {
  return {
    schemaVersion: 1,
    loaderKey: "account/worker/version",
    workerCodeSha256: "a".repeat(64),
    routeGeneration: 1,
    compatibilityDate: "2026-08-30",
    compatibilityFlags: [],
    contentKind: "worker",
    mainModule: "index.js",
    modules: [],
    moduleBindings: [],
    env: {},
    bindings: [],
    scheduledTargets: [],
    services: [{
      schemaVersion: 1,
      name: "CATALOG",
      targetWorkerId: "worker",
      props,
      policyVersion: 1,
      descriptorSha256: "b".repeat(64),
    }],
    cachePolicy: { enabled: false, failOpen: false, crossVersionCache: false, entrypoints: {} },
  };
}

test("accepts bounded arbitrary JSON Service props", () => {
  const value = snapshot(JSON.parse(
    '{"constructor":{"enabled":true},"nested":[1,{"__proto__":"ordinary JSON data"}]}',
  ));
  assert.doesNotThrow(() => assertSnapshot(value));
});

test("rejects non-object, over-depth, and oversized Service props", () => {
  assert.throws(() => assertSnapshot(snapshot([])), /VERSION_INVARIANT_VIOLATION/);
  let nested = true;
  for (let index = 0; index < 33; index += 1) nested = [nested];
  assert.throws(() => assertSnapshot(snapshot({ nested })), /VERSION_INVARIANT_VIOLATION/);
  assert.throws(
    () => assertSnapshot(snapshot({ value: "x".repeat(64 * 1024) })),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

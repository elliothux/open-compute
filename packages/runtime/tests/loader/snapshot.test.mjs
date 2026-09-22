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
    compatibilityDate: "2026-09-08",
    compatibilityFlags: [],
    limits: { cpuMs: 30000, subRequests: 10000 },
    contentKind: "worker",
    mainModule: "index.js",
    modules: [],
    moduleBindings: [],
    workerLoaders: [],
    env: {},
    bindings: [],
    scheduledTargets: [],
    services: [
      {
        schemaVersion: 2,
        name: "CATALOG",
        target: { kind: "worker", workerId: "worker" },
        props,
        policyVersion: 1,
        descriptorSha256: "b".repeat(64),
      },
    ],
    cachePolicy: {
      enabled: false,
      failOpen: false,
      crossVersionCache: false,
      entrypoints: {},
    },
  };
}

test("accepts bounded arbitrary JSON Service props", () => {
  const value = snapshot(
    JSON.parse(
      '{"constructor":{"enabled":true},"nested":[1,{"__proto__":"ordinary JSON data"}]}',
    ),
  );
  assert.doesNotThrow(() => assertSnapshot(value));
});

test("rejects non-object, over-depth, and oversized Service props", () => {
  assert.throws(
    () => assertSnapshot(snapshot([])),
    /VERSION_INVARIANT_VIOLATION/,
  );
  let nested = true;
  for (let index = 0; index < 33; index += 1) nested = [nested];
  assert.throws(
    () => assertSnapshot(snapshot({ nested })),
    /VERSION_INVARIANT_VIOLATION/,
  );
  assert.throws(
    () => assertSnapshot(snapshot({ value: "x".repeat(64 * 1024) })),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("native Loader snapshot rejects malformed or aliased namespace authority", () => {
  const binding = {
    name: "LOADER",
    namespaceKey: `${"c".repeat(64)}/${"0".repeat(15)}1/${"d".repeat(64)}`,
  };
  assert.doesNotThrow(() =>
    assertSnapshot({ ...snapshot({}), workerLoaders: [binding] }),
  );
  for (const workerLoaders of [
    undefined,
    {},
    [null],
    [{ ...binding, name: "__PRIVATE" }],
    [{ ...binding, namespaceKey: "tenant-key" }],
    [binding, binding],
    [binding, { ...binding, name: "OTHER" }],
  ]) {
    assert.throws(
      () => assertSnapshot({ ...snapshot({}), workerLoaders }),
      /VERSION_INVARIANT_VIOLATION/,
    );
  }
});

test("tenant modules cannot occupy the platform module namespace", () => {
  const runtimeModule = {
    name: "open-compute:worker-loader",
    type: "esModule",
    bytesBase64: "",
  };
  assert.throws(
    () => assertSnapshot({ ...snapshot({}), modules: [runtimeModule] }),
    /VERSION_INVARIANT_VIOLATION/,
  );
  assert.doesNotThrow(() =>
    assertSnapshot({
      ...snapshot({}),
      modules: [{ ...runtimeModule, name: "app/worker-loader.js" }],
    }),
  );
});

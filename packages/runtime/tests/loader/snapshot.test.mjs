import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { assertSnapshot } = await importRuntime("loader/snapshot.ts");

function snapshot(props) {
  return {
    schemaVersion: 1,
    loaderKey:
      "019c0000000070008000000000000001/019c0000-0000-7000-8000-000000000002/019c0000-0000-7000-8000-000000000003",
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
    browserBindings: [],
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
      '{"constructor":{"enabled":true},"pythonSnapshot":"ordinary JSON data","pythonPreparedSha256":123,"nested":[1,{"__proto__":"ordinary JSON data"}]}',
    ),
  );
  assert.doesNotThrow(() => assertSnapshot(value));
});

test("Python source wire uses the single current module representation", () => {
  const value = snapshot({});
  value.mainModule = "entry.py";
  value.modules = [
    { name: "entry.py", type: "python", bytesBase64: "cGFzcw==" },
  ];
  assert.doesNotThrow(() => assertSnapshot(value));
  for (const type of ["py", "pythonModule", "python-requirement"]) {
    value.modules[0].type = type;
    assert.throws(() => assertSnapshot(value), /VERSION_INVARIANT_VIOLATION/);
  }
});

test("private source JSON accepts only Python artifact identity, never snapshot bytes", () => {
  const value = {
    ...snapshot({}),
    mainModule: "main.py",
    pythonPreparedSha256: "a".repeat(64),
  };
  assert.doesNotThrow(() => assertSnapshot(value));
  for (const invalid of [
    { pythonPreparedSha256: "A".repeat(64) },
    { pythonPreparedSha256: 123 },
    { mainModule: "main.js" },
    { contentKind: "assetsOnly" },
    { pythonSnapshot: "c2VjcmV0" },
    { pythonSnapshot: null },
  ])
    assert.throws(
      () => assertSnapshot({ ...value, ...invalid }),
      /VERSION_INVARIANT_VIOLATION/,
    );
});

test("observability snapshot requires the instance identity field", () => {
  const observability = {
    schemaVersion: 1,
    instanceId: "019c0000000070008000000000000001",
    workerId: "019c0000-0000-7000-8000-000000000002",
    scriptName: "worker",
    versionId: "019c0000-0000-7000-8000-000000000003",
    routeGeneration: 1,
    observabilityGeneration: 1,
    enabled: true,
    logsEnabled: true,
    headSamplingRate: 1,
    invocationLogs: true,
    persist: true,
  };
  assert.doesNotThrow(() => assertSnapshot({ ...snapshot({}), observability }));
  const { instanceId, ...withoutInstanceId } = observability;
  assert.ok(instanceId);
  assert.throws(
    () =>
      assertSnapshot({
        ...snapshot({}),
        observability: { ...withoutInstanceId, accountId: instanceId },
      }),
    /VERSION_INVARIANT_VIOLATION/,
  );
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

test("tenant modules cannot replace a private config-owned extension", () => {
  assert.throws(
    () =>
      assertSnapshot({
        ...snapshot({}),
        modules: [
          {
            name: "cloudflare-internal:open-compute-host-policy",
            type: "esModule",
            bytesBase64: "",
          },
        ],
      }),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

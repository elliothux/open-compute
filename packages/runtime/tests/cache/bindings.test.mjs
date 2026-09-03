import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const host = moduleUrl(`
  export function bindingError(code) { return Object.assign(new Error(code), { stableCode: code }); }
`);
const { tenantEnv } = await importRuntime("loader/bindings.ts", { "./host.js": host });

const snapshot = {
  loaderKey: "account/worker/version",
  routeGeneration: 7,
  workerCodeSha256: "ab".repeat(32),
  env: { PUBLIC: "value" },
  bindings: [],
  moduleBindings: [],
  services: [],
  cachePolicy: {
    enabled: false,
    crossVersionCache: false,
    failOpen: true,
    entrypoints: { Admin: { enabled: true, crossVersionCache: true } },
  },
};

test("tenant env creates a cache transport for the current unconfigured entrypoint", () => {
  const transports = [];
  const ctx = {
    exports: {
      CacheTransport({ props }) { transports.push(props); return props; },
    },
  };
  const env = tenantEnv(snapshot, ctx, "version", {}, false, true, "Named");
  assert.deepEqual(Object.keys(env.__OPEN_COMPUTE_PRIVATE_CACHE).sort(), ["Admin", "Named", "default"]);
  assert.deepEqual(transports.map(value => [
    value.entrypoint, value.automaticEnabled, value.crossVersionCache,
  ]).sort(), [
    ["Admin", true, true],
    ["Named", false, false],
    ["default", false, false],
  ]);
  assert.equal(env.PUBLIC, "value");
});

test("tenant env resolves AI from the immutable version descriptor", () => {
  let received;
  const configured = {
    ...snapshot,
    aiBinding: { name: "AI", descriptorSha256: "cd".repeat(32) },
  };
  const env = tenantEnv(configured, { exports: {
    CacheTransport({ props }) { return props; },
    AiTransport({ props }) { received = props; return { transform() {}, supported() {} }; },
  } }, "version", {}, false, true);
  assert.deepEqual(received, {
    accountId: "account", workerId: "worker", versionId: "version",
    descriptorSha256: "cd".repeat(32),
  });
  assert.equal(typeof env.AI.transform, "function");
});

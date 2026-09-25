import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const shared = moduleUrl(`
  export function bindingError(code) { return Object.assign(new Error(code), { stableCode: code }); }
`);
const { tenantEnv } = await importRuntime("loader/bindings.ts", {
  "./shared.js": shared,
});

const snapshot = {
  loaderKey: "account/worker/version",
  routeGeneration: 7,
  workerCodeSha256: "ab".repeat(32),
  env: { PUBLIC: "value" },
  bindings: [],
  moduleBindings: [],
  workerLoaders: [],
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
      CacheTransport({ props }) {
        transports.push(props);
        return props;
      },
    },
  };
  const { env, openComputePrivateEnv } = tenantEnv(
    snapshot,
    ctx,
    {},
    "version",
    {},
    false,
    "Named",
  );
  assert.deepEqual(
    Object.keys(openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_CACHE).sort(),
    ["Admin", "Named", "default"],
  );
  assert.deepEqual(
    transports
      .map((value) => [
        value.entrypoint,
        value.automaticEnabled,
        value.crossVersionCache,
      ])
      .sort(),
    [
      ["Admin", true, true],
      ["Named", false, false],
      ["default", false, false],
    ],
  );
  assert.equal(env.PUBLIC, "value");
});

test("tenant env resolves AI from the immutable version descriptor", () => {
  let received;
  const configured = {
    ...snapshot,
    aiBinding: { name: "AI", descriptorSha256: "cd".repeat(32) },
  };
  const { env, openComputePrivateEnv } = tenantEnv(
    configured,
    {
      exports: {
        CacheTransport({ props }) {
          return props;
        },
        AiTransport({ props }) {
          received = props;
          return { transform() {}, supported() {} };
        },
      },
    },
    {},
    "version",
    {},
    false,
    true,
  );
  assert.deepEqual(received, {
    instanceId: "account",
    workerId: "worker",
    versionId: "version",
    descriptorSha256: "cd".repeat(32),
  });
  assert.equal(env.AI, undefined);
  assert.equal(typeof openComputePrivateEnv.AI.transform, "function");
});

test("DO and Workflow env keep programmatic cache and declared built-ins", () => {
  const configured = {
    ...snapshot,
    imagesBinding: { name: "IMAGES", descriptorSha256: "bc".repeat(32) },
    aiBinding: { name: "AI", descriptorSha256: "cd".repeat(32) },
    versionMetadataBinding: {
      name: "VERSION",
      id: "version",
      tag: "context-matrix",
      timestampMs: 10,
    },
  };
  for (const context of [
    { durableObject: true, entrypoint: "Object" },
    { durableObject: false, entrypoint: "Flow" },
  ]) {
    const cacheProps = [];
    const { env, openComputePrivateEnv } = tenantEnv(
      configured,
      {
        exports: {
          CacheTransport({ props }) {
            cacheProps.push(props);
            return props;
          },
          ImageTransport({ props }) {
            return { kind: "images", props };
          },
          AiTransport({ props }) {
            return { kind: "ai", props };
          },
        },
      },
      {},
      "version",
      {},
      context.durableObject,
      context.entrypoint,
    );
    assert.equal(
      openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_CACHE[context.entrypoint]
        .entrypoint,
      context.entrypoint,
    );
    assert.equal(
      cacheProps.some((props) => props.entrypoint === context.entrypoint),
      true,
    );
    assert.equal(env.IMAGES, undefined);
    assert.equal(env.AI, undefined);
    assert.equal(openComputePrivateEnv.IMAGES.kind, "images");
    assert.equal(openComputePrivateEnv.AI.kind, "ai");
    assert.deepEqual(env.VERSION, {
      id: "version",
      tag: "context-matrix",
      timestamp: "1970-01-01T00:00:00.010Z",
    });
  }
});

test("tenant env receives only native loader capabilities from verified descriptors", () => {
  const capability = Object.freeze({ load() {}, get() {} });
  const privateCapability = Object.freeze({ load() {}, get() {} });
  const keys = [];
  const factory = {
    get(key) {
      keys.push(key);
      return capability;
    },
    getPrivate(key) {
      keys.push(`private:${key}`);
      return privateCapability;
    },
  };
  const configured = {
    ...snapshot,
    workerLoaders: [
      { name: "LOADER", namespaceKey: "private-authority" },
      { name: "OTHER", namespaceKey: "other-authority" },
    ],
  };
  const ctx = {
    exports: {
      CacheTransport({ props }) {
        return props;
      },
    },
  };
  const { env, openComputePrivateEnv } = tenantEnv(
    configured,
    ctx,
    factory,
    "version",
    {},
    false,
  );
  assert.deepEqual(keys, [
    "private-authority",
    "private:private-authority",
    "other-authority",
    "private:other-authority",
  ]);
  assert.equal(env.LOADER, capability);
  assert.deepEqual(Object.keys(env).sort(), ["LOADER", "OTHER", "PUBLIC"]);
  assert.deepEqual(Object.keys(openComputePrivateEnv).sort(), [
    "__OPEN_COMPUTE_PRIVATE_CACHE",
    "__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS",
  ]);
  assert.equal(
    openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS.LOADER,
    privateCapability,
  );
  assert.equal(
    openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS.OTHER,
    privateCapability,
  );
  assert.throws(
    () =>
      tenantEnv(
        { ...configured, env: { LOADER: "conflict" } },
        ctx,
        factory,
        "version",
        {},
        false,
      ),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("product transport is handler-only while declared values stay importable", () => {
  const transport = { get() {} };
  const configured = {
    ...snapshot,
    bindings: [
      {
        kind: "kv_namespace",
        name: "KV",
        capabilityVersion: 1,
        bindingId: "binding",
        descriptorSha256: "c".repeat(64),
        resourceId: "resource",
        resourceSpecGeneration: 1,
        permissions: { read: true, write: false },
      },
    ],
  };
  const code = tenantEnv(
    configured,
    {
      exports: {
        KVNamespace() {
          return transport;
        },
        CacheTransport() {
          return {};
        },
      },
    },
    {},
    "version",
    {},
    false,
  );
  assert.deepEqual(Object.keys(code.env), ["PUBLIC"]);
  assert.equal(code.env.PUBLIC, "value");
  assert.equal(code.openComputePrivateEnv.KV, transport);
});

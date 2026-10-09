import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const shared = moduleUrl(`
  export function bindingError(code) { return Object.assign(new Error(code), { stableCode: code }); }
`);
const { tenantEnv, validationEnv } = await importRuntime("loader/bindings.ts", {
  "./shared.js": shared,
  "./policy.js": moduleUrl(await compileRuntime("loader/policy.ts")),
  "../bindings/native-construction.js": moduleUrl(
    await compileRuntime("bindings/native-construction.ts"),
  ),
});

const snapshot = {
  loaderKey: "account/worker/version",
  routeGeneration: 7,
  workerCodeSha256: "ab".repeat(32),
  env: { PUBLIC: "value" },
  bindings: [],
  moduleBindings: [],
  workerLoaders: [],
  browserBindings: [],
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
  const { env, openComputePrivateEnv, openComputeCache } = tenantEnv(
    snapshot,
    {},
    ctx,
    { hostPolicyVersion: 1 },
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
  assert.equal(
    openComputeCache,
    openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_CACHE.Named,
  );
  assert.equal(env.openComputeCache, undefined);
});

test("tenant env resolves AI from the immutable version descriptor", () => {
  let received;
  const configured = {
    ...snapshot,
    aiBinding: { name: "AI", descriptorSha256: "cd".repeat(32) },
  };
  const { env, openComputePrivateEnv, openComputeBindings } = tenantEnv(
    configured,
    {},
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
    { hostPolicyVersion: 1 },
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
  assert.equal(openComputePrivateEnv.AI, undefined);
  assert.equal(
    openComputeBindings.AI.wrapperModule,
    "cloudflare-internal:open-compute-ai",
  );
  assert.equal(typeof openComputeBindings.AI.fetcher.transform, "function");
});

test("DO and Workflow env keep programmatic cache and declared built-ins", () => {
  const configured = {
    ...snapshot,
    assetBinding: { name: "ASSETS" },
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
    const { env, openComputePrivateEnv, openComputeBindings } = tenantEnv(
      configured,
      {},
      {
        exports: {
          CacheTransport({ props }) {
            cacheProps.push(props);
            return props;
          },
          ImageTransport({ props }) {
            return { kind: "images", props };
          },
          AssetTransport({ props }) {
            return { kind: "assets", props };
          },
          AiTransport({ props }) {
            return { kind: "ai", props };
          },
        },
      },
      { hostPolicyVersion: 1 },
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
    assert.equal(env.ASSETS, undefined);
    assert.equal(openComputePrivateEnv.ASSETS, undefined);
    assert.equal(openComputeBindings.ASSETS.fetcher.kind, "assets");
    assert.deepEqual(openComputeBindings.ASSETS.fetcher.props, {
      versionId: "version",
      descriptorSha256: snapshot.workerCodeSha256,
    });
    assert.equal(
      openComputeBindings.ASSETS.wrapperModule,
      "cloudflare-internal:open-compute-assets",
    );
    assert.equal(openComputePrivateEnv.IMAGES, undefined);
    assert.equal(openComputePrivateEnv.AI, undefined);
    assert.equal(openComputeBindings.IMAGES.fetcher.kind, "images");
    assert.equal(
      openComputeBindings.IMAGES.wrapperModule,
      "cloudflare-internal:open-compute-images",
    );
    assert.equal(openComputeBindings.AI.fetcher.kind, "ai");
    assert.equal(
      openComputeBindings.AI.wrapperModule,
      "cloudflare-internal:open-compute-ai",
    );
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
    hostPolicyVersion: 1,
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
  const { env, openComputePrivateEnv, openComputeBindings } = tenantEnv(
    configured,
    {},
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
    "__OPEN_COMPUTE_PRIVATE_NATIVE_BINDINGS",
    "__OPEN_COMPUTE_PRIVATE_POLICY",
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
        {},
        ctx,
        factory,
        "version",
        {},
        false,
      ),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("native binding construction preserves the descriptor and keeps raw transports private", () => {
  const transport = { get() {} };
  const serviceTransport = { fetch() {}, rpc() {} };
  const service = {
    name: "SERVICE",
    descriptorSha256: "d".repeat(64),
    entrypoint: "Named",
  };
  const configured = {
    ...snapshot,
    services: [service],
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
  const ctx = {
    exports: {
      KVNamespace() {
        return transport;
      },
      ServiceTransport({ props }) {
        assert.ok(Object.isFrozen(props));
        assert.deepEqual(props, {
          versionId: "version",
          bindingName: "SERVICE",
          descriptorSha256: service.descriptorSha256,
          entrypoint: "Named",
        });
        return serviceTransport;
      },
      CacheTransport() {
        return {};
      },
    },
  };
  const code = tenantEnv(
    configured,
    {},
    ctx,
    { hostPolicyVersion: 1 },
    "version",
    {},
    false,
  );
  assert.deepEqual(Object.keys(code.env), ["PUBLIC"]);
  assert.equal(code.env.PUBLIC, "value");
  assert.equal(code.openComputePrivateEnv.KV, undefined);
  assert.deepEqual(code.openComputeBindings.KV, {
    kind: "kvNamespace",
    fetcher: transport,
  });
  assert.equal(code.openComputePrivateEnv.SERVICE, undefined);
  assert.deepEqual(code.openComputeBindings.SERVICE, {
    kind: "service",
    fetcher: serviceTransport,
  });
  assert.throws(
    () =>
      tenantEnv(
        { ...configured, services: [{ ...service, name: "PUBLIC" }] },
        {},
        ctx,
        { hostPolicyVersion: 1 },
        "version",
        {},
      ),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("host capability assembly refuses a runtime without native host policy", () => {
  for (const factory of [
    {},
    { hostPolicyVersion: 0 },
    { hostPolicyVersion: 2 },
  ]) {
    assert.throws(
      () => validationEnv(snapshot, {}, factory),
      /RUNTIME_UNAVAILABLE/,
    );
    assert.throws(
      () => tenantEnv(snapshot, {}, {}, factory, "version", {}),
      /RUNTIME_UNAVAILABLE/,
    );
  }
  assert.equal(
    validationEnv(snapshot, {}, { hostPolicyVersion: 1 }).openComputeHostPolicy,
    true,
  );
});

test("Queue construction preserves its private publication transport in every caller context", () => {
  const descriptor = {
    name: "EVENTS",
    kind: "queue_producer",
    capabilityVersion: 1,
    bindingId: "binding",
    descriptorSha256: "ab".repeat(32),
    queueId: "queue",
    queueLifecycleGeneration: 1,
  };
  for (const durableObject of [false, true]) {
    const transport = {};
    let received;
    const code = tenantEnv(
      { ...snapshot, bindings: [descriptor] },
      {},
      {
        exports: {
          CacheTransport() {
            return {};
          },
          QueueTransport({ props }) {
            received = props;
            return transport;
          },
        },
      },
      { hostPolicyVersion: 1 },
      "version",
      {},
      durableObject,
    );
    assert.deepEqual(code.openComputeBindings.EVENTS, {
      kind: "queue",
      fetcher: transport,
    });
    assert.equal(code.openComputePrivateEnv.EVENTS, transport);
    assert.equal(code.env.EVENTS, undefined);
    assert.equal(received.durableObject, durableObject);
    assert.equal(received.bindingId, descriptor.bindingId);
    assert.equal(received.queueId, descriptor.queueId);
    assert.equal(received.versionId, "version");
  }
});

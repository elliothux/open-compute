import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const source = await compileRuntime("loader/forwarding.ts", {
  "./policy.js": moduleUrl(await compileRuntime("loader/policy.ts")),
  "../bindings/native-construction.js": moduleUrl(
    await compileRuntime("bindings/native-construction.ts"),
  ),
});
const forwardingModule = await import(moduleUrl(source));
const { createForwarding } = forwardingModule;
const { getWorker, loadWorker } = createForwarding({
  get(id, callback) {
    return this.get(id, callback);
  },
  load(code) {
    return this.load(code);
  },
});

function input(value) {
  return {
    compatibilityDate: "2026-09-08",
    mainModule: "main.js",
    modules: { "main.js": { js: "export default { fetch() {} };" } },
    env: { VALUE: 7, KV: value },
  };
}

test("forwarded native root reconstructs its capability without changing native get laziness", async () => {
  const root = {};
  const transport = { get() {} };
  const owner = {};
  let loaded;
  const loader = {
    get(id, callback) {
      loaded = { id, callback };
      return { cached: true };
    },
  };
  const roots = new WeakMap([
    [
      root,
      {
        kind: "binding",
        descriptor: { kind: "kv_namespace", name: "OLD", capabilityVersion: 1 },
        transport,
        owner,
      },
    ],
  ]);
  let calls = 0;
  const stub = getWorker(
    loader,
    "code-1",
    () => {
      calls++;
      return input(root);
    },
    roots,
    "source-1",
    owner,
  );
  assert.equal(stub.cached, true);
  assert.equal(loaded.id, "source-1/code-1");
  assert.equal(calls, 0);
  const code = await loaded.callback();
  assert.equal(calls, 1);
  assert.equal(code.env.VALUE, 7);
  assert.equal("KV" in code.env, false);
  assert.equal(code.openComputePrivateEnv.KV, undefined);
  assert.deepEqual(code.openComputeBindings.KV, {
    kind: "kvNamespace",
    fetcher: transport,
  });
  assert.doesNotMatch(
    code.modules[code.mainModule].js,
    /KVNamespace|kv\/facade/,
  );
  assert.doesNotMatch(code.modules[code.mainModule].js, /export \* from/);
  assert.deepEqual(Object.keys(code.modules), ["main.js"]);
  assert.equal(code.mainModule, "main.js");
  assert.equal(code.openComputeHostPolicy, true);
  assert.ok(code.modules["main.js"]);
});

test("getWorker keeps native cache-hit laziness, so one ID names one immutable snapshot", async () => {
  const cache = new Map();
  const loader = {
    get(id, callback) {
      if (!cache.has(id)) cache.set(id, { id, callback });
      return cache.get(id);
    },
  };
  const owner = {};
  const roots = new WeakMap();
  let firstCalls = 0;
  let secondCalls = 0;
  const first = getWorker(
    loader,
    "stable-id",
    () => {
      firstCalls++;
      return input(1);
    },
    roots,
    "source-1",
    owner,
  );
  const second = getWorker(
    loader,
    "stable-id",
    () => {
      secondCalls++;
      return input(2);
    },
    roots,
    "source-1",
    owner,
  );
  assert.strictEqual(second, first);
  assert.equal(firstCalls, 0);
  assert.equal(secondCalls, 0);
  const code = await first.callback();
  assert.equal(code.env.KV, 1);
  assert.equal(firstCalls, 1);
  assert.equal(secondCalls, 0);
});

test("unregistered instances and reserved modules cannot claim a forwarded binding", () => {
  class FakeBinding {}
  const root = {};
  const owner = {};
  let captured;
  const loader = {
    load(code) {
      captured = code;
      return code;
    },
  };
  const roots = new WeakMap([
    [
      root,
      {
        kind: "binding",
        descriptor: { kind: "kv_namespace", name: "KV", capabilityVersion: 1 },
        transport: {},
        owner,
      },
    ],
  ]);
  loadWorker(loader, input(new FakeBinding()), roots, owner);
  assert.equal(captured.env.KV instanceof FakeBinding, true);
  assert.deepEqual(Object.keys(captured.openComputePrivateEnv), [
    "__OPEN_COMPUTE_PRIVATE_POLICY",
  ]);
  assert.throws(
    () =>
      loadWorker(
        loader,
        {
          ...input(root),
          modules: { "cloudflare-internal:open-compute-host-policy": "bad" },
        },
        roots,
        owner,
      ),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
  for (const name of [
    "./main.js",
    "tenant/../main.js",
    "tenant\\..\\main.js",
  ]) {
    assert.throws(
      () =>
        loadWorker(
          loader,
          { ...input(root), modules: { [name]: "bad" } },
          roots,
          owner,
        ),
      /WORKER_LOADER_FORWARDING_DENIED/,
    );
  }
  assert.throws(
    () =>
      loadWorker(
        loader,
        { ...input(root), mainModule: "tenant/../main.js" },
        roots,
        owner,
      ),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
  assert.throws(
    () =>
      loadWorker(loader, { ...input(root), env: { __KV: root } }, roots, owner),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
  assert.throws(
    () => loadWorker({ load: (value) => value }, input(root), roots, {}),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
});

test("product roots use the single native or wrapped implementation", () => {
  const kinds = [
    "kv_namespace",
    "r2_bucket",
    "d1_database",
    "do_namespace",
    "queue_producer",
    "workflow",
    "vectorize_index",
    "ai_search_namespace",
    "ai_search_instance",
    "artifacts_namespace",
  ];
  const roots = new WeakMap();
  const owner = {};
  const loader = { load: (value) => value };
  const env = {};
  for (const [index, kind] of kinds.entries()) {
    const name = `B${index}`;
    const facade = {};
    roots.set(facade, {
      kind: "binding",
      descriptor: { kind, name, capabilityVersion: 1 },
      transport: `${kind}-transport`,
      owner,
    });
    env[name] = facade;
  }
  for (const kind of ["service", "assets", "images", "ai"]) {
    const facade = {};
    roots.set(facade, {
      kind,
      ...(kind === "service"
        ? { descriptor: { name: "OLD", schemaVersion: 2 } }
        : {}),
      transport: `${kind}-transport`,
      owner,
    });
    env[kind.toUpperCase()] = facade;
  }
  const code = loadWorker(loader, { ...input(undefined), env }, roots, owner);
  assert.deepEqual(Object.keys(code.env), []);
  assert.equal(Object.keys(code.openComputePrivateEnv).length, 4);
  assert.equal(Object.keys(code.openComputeBindings).length, 12);
  assert.equal(code.openComputeBindings.SERVICE.kind, "service");
  assert.equal(code.openComputeBindings.B4.kind, "queue");
  for (const [name, module] of [
    ["B9", "artifacts"],
    ["B7", "ai-search-namespace"],
    ["B8", "ai-search-instance"],
    ["ASSETS", "assets"],
    ["IMAGES", "images"],
    ["AI", "ai"],
  ]) {
    assert.equal(code.openComputeBindings[name].kind, "wrapped");
    assert.equal(
      code.openComputeBindings[name].wrapperModule,
      `cloudflare-internal:open-compute-${module}`,
    );
    assert.equal(code.openComputePrivateEnv[name], undefined);
  }
  for (const name of Object.keys(env)) {
    const native = code.openComputeBindings[name];
    if (native) {
      assert.equal(native.fetcher, roots.get(env[name]).transport);
      assert.equal(
        code.openComputePrivateEnv[name],
        native.kind === "queue" ? native.fetcher : undefined,
      );
      if (name === "B2")
        assert.equal(native.wrapperModule, "cloudflare-internal:d1-api");
    } else {
      assert.equal(
        code.openComputePrivateEnv[name],
        roots.get(env[name]).transport,
      );
      assert.ok(
        code.openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_POLICY.bindings.some(
          (binding) => binding.name === name,
        ) ||
          code.openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_POLICY.services.some(
            (service) => service.name === name,
          ),
      );
    }
  }
});

test("tenant prototype edits cannot forge a registered root or replace native Loader calls", () => {
  const owner = {};
  const root = {};
  const roots = new WeakMap();
  const loaderPrototype = {
    load(code) {
      return code;
    },
    get(id, callback) {
      return { id, callback };
    },
  };
  const loader = Object.create(loaderPrototype);
  const safe = createForwarding({
    get: loaderPrototype.get,
    load: loaderPrototype.load,
  });
  const originalWeakGet = WeakMap.prototype.get;
  const originalLoad = loaderPrototype.load;
  try {
    WeakMap.prototype.get = () => ({
      kind: "binding",
      descriptor: { kind: "kv_namespace", name: "KV", capabilityVersion: 1 },
      transport: "forged-transport",
      owner,
    });
    loaderPrototype.load = () => {
      throw new Error("private Loader grant intercepted");
    };
    const code = safe.loadWorker(loader, input(root), roots, owner);
    assert.equal(code.env.KV, root);
    assert.equal(code.openComputePrivateEnv.KV, undefined);
  } finally {
    WeakMap.prototype.get = originalWeakGet;
    loaderPrototype.load = originalLoad;
  }
});

// Host-only construction and snapshots must never come from child tenant code.
test("forwarding rejects every tenant supplied host field before native load", () => {
  let loads = 0;
  const loader = {
    load() {
      loads++;
    },
  };
  for (const name of [
    "openComputeBindings",
    "openComputePrivateEnv",
    "openComputePythonSnapshot",
    "openComputeHostPolicy",
    "openComputeFutureGrant",
  ]) {
    const code = input(1);
    Object.defineProperty(code, name, { value: {}, enumerable: false });
    assert.throws(
      () => loadWorker(loader, code, new WeakMap(), {}),
      /WORKER_LOADER_FORWARDING_DENIED/,
    );
  }
  assert.equal(loads, 0);
});

test("forwarding snapshots mutable tenant code and seals inherited host fields", () => {
  let captured;
  const loader = {
    load(code) {
      captured = code;
      return code;
    },
  };
  const code = input(1);
  Object.defineProperty(code.modules, "extra.js", {
    enumerable: true,
    get() {
      Object.defineProperty(code, "openComputePythonSnapshot", {
        value: new Uint8Array(16),
        enumerable: true,
      });
      return { js: "export {};" };
    },
  });
  const original = Object.getOwnPropertyDescriptor(
    Object.prototype,
    "openComputePythonSnapshot",
  );
  try {
    Object.defineProperty(Object.prototype, "openComputePythonSnapshot", {
      value: new Uint8Array(16),
      configurable: true,
    });
    loadWorker(loader, code, new WeakMap(), {});
    assert.equal(Object.getPrototypeOf(captured), null);
    assert.equal(captured.openComputePythonSnapshot, undefined);
    assert.equal(Object.hasOwn(code, "openComputePythonSnapshot"), true);
  } finally {
    if (original)
      Object.defineProperty(
        Object.prototype,
        "openComputePythonSnapshot",
        original,
      );
    else delete Object.prototype.openComputePythonSnapshot;
  }
  let keys = 0;
  const malicious = new Proxy(
    { ...input(1), openComputePythonSnapshot: new Uint8Array(16) },
    {
      ownKeys(target) {
        return ++keys === 1
          ? Reflect.ownKeys(target)
          : Reflect.ownKeys(target).filter(
              (name) => name !== "openComputePythonSnapshot",
            );
      },
    },
  );
  assert.throws(
    () => loadWorker(loader, malicious, new WeakMap(), {}),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
});

test("only the INTERNAL host registry connects public Loaders to private forwarding grants", async () => {
  const { registerForwarding, forwardGetWorker, forwardLoadWorker } =
    await import(moduleUrl(source));
  const publicLoader = {};
  const otherPublicLoader = {};
  const calls = [];
  const prototype = {
    get(id, callback) {
      calls.push(this.namespace);
      return { id, callback };
    },
    load(code) {
      return code;
    },
  };
  const privateLoader = Object.assign(Object.create(prototype), {
    namespace: "LOADER",
  });
  const otherPrivateLoader = Object.assign(Object.create(prototype), {
    namespace: "OTHER",
  });
  const nativeKv = {};
  const transport = {};
  const owner = { LOADER: privateLoader, OTHER: otherPrivateLoader };
  const publicEnvironment = {
    LOADER: publicLoader,
    OTHER: otherPublicLoader,
    KV: nativeKv,
  };
  const privateEnvironment = {
    __OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS: owner,
    __OPEN_COMPUTE_PRIVATE_NATIVE_BINDINGS: { KV: { fetcher: transport } },
  };
  const policy = {
    sourceIdentity: "source-1",
    browserBindingNames: [],
    workerLoaderNames: ["LOADER", "OTHER"],
    bindings: [{ kind: "kv_namespace", name: "KV", capabilityVersion: 1 }],
    services: [],
  };
  registerForwarding(publicEnvironment, privateEnvironment, policy);
  const code = input(nativeKv);
  for (const loader of [publicLoader, otherPublicLoader]) {
    const stub = forwardGetWorker(loader, "child", () => code);
    assert.equal(stub.id, "source-1/child");
    assert.equal(
      (await stub.callback()).openComputeBindings.KV.fetcher,
      transport,
    );
    assert.equal(
      forwardLoadWorker(loader, code).openComputeBindings.KV.fetcher,
      transport,
    );
  }
  assert.deepEqual(calls, ["LOADER", "OTHER"]);
  assert.throws(
    () => forwardGetWorker({}, "child", () => code),
    /FORWARDING_DENIED/,
  );
  assert.throws(() => forwardLoadWorker({}, code), /FORWARDING_DENIED/);
  assert.equal(
    forwardLoadWorker(publicLoader, input({ forged: true })).openComputeBindings
      .KV,
    undefined,
  );
  const anotherKv = {};
  const anotherLoader = {};
  registerForwarding(
    { LOADER: anotherLoader, KV: anotherKv },
    {
      __OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS: { LOADER: privateLoader },
      __OPEN_COMPUTE_PRIVATE_NATIVE_BINDINGS: { KV: { fetcher: {} } },
    },
    { ...policy, browserBindingNames: [], workerLoaderNames: ["LOADER"] },
  );
  assert.equal(
    forwardLoadWorker(otherPublicLoader, input(anotherKv)).openComputeBindings
      .KV,
    undefined,
  );
  assert.equal(
    forwardLoadWorker(anotherLoader, code).openComputeBindings.KV,
    undefined,
  );
});

test("host forwarding registry rejects incomplete authority and registers all declared roots", () => {
  const { registerForwarding, forwardLoadWorker } = forwardingModule;
  const policy = {
    sourceIdentity: "source-1",
    browserBindingNames: [],
    workerLoaderNames: ["LOADER"],
    bindings: [],
    services: [],
  };
  const prototype = {
    load(code) {
      return code;
    },
    get() {},
  };
  const loader = Object.create(prototype);
  const publicLoader = {};
  const owner = { LOADER: loader };
  const authority = { __OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS: owner };
  for (const [environment, privateEnvironment, selected] of [
    [{}, {}, policy],
    [
      {},
      authority,
      { ...policy, browserBindingNames: [], workerLoaderNames: [] },
    ],
    [{}, authority, policy],
    [
      { LOADER: publicLoader },
      { __OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS: {} },
      policy,
    ],
    [
      { LOADER: publicLoader },
      {
        __OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS: {
          LOADER: Object.create(null),
        },
      },
      policy,
    ],
    [
      { LOADER: publicLoader },
      { __OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS: { LOADER: {} } },
      policy,
    ],
    [
      { LOADER: publicLoader },
      authority,
      { ...policy, bindings: [{ name: "MISSING" }] },
    ],
  ])
    assert.throws(
      () => registerForwarding(environment, privateEnvironment, selected),
      /FORWARDING_DENIED/,
    );
  assert.equal(
    registerForwarding({}, {}, { ...policy, sourceIdentity: undefined }),
    undefined,
  );
  const env = {
    LOADER: publicLoader,
    SERVICE: {},
    ASSETS: {},
    IMAGES: {},
    AI: {},
  };
  const transports = { SERVICE: {}, ASSETS: {}, IMAGES: {}, AI: {} };
  registerForwarding(
    env,
    { ...authority, ...transports },
    {
      ...policy,
      services: [{ name: "SERVICE" }],
      assetBindingName: "ASSETS",
      imagesBindingName: "IMAGES",
      aiBindingName: "AI",
    },
  );
  const code = forwardLoadWorker(publicLoader, {
    ...input(undefined),
    env: { ...env, LOADER: undefined },
  });
  assert.equal(code.openComputePrivateEnv.SERVICE, undefined);
  assert.equal(code.openComputeBindings.SERVICE.kind, "service");
  assert.equal(code.openComputeBindings.SERVICE.fetcher, transports.SERVICE);
  for (const kind of ["ASSETS", "IMAGES", "AI"])
    assert.equal(code.openComputeBindings[kind].fetcher, transports[kind]);
});

test("forwarded Python uses native baseline code with only selected binding roots", async () => {
  const root = {};
  const owner = {};
  const transport = {};
  const roots = new WeakMap([
    [
      root,
      {
        kind: "binding",
        descriptor: { kind: "kv_namespace", name: "KV", capabilityVersion: 1 },
        transport,
        owner,
      },
    ],
  ]);
  const python =
    "from workers import WorkerEntrypoint, env\nclass Default(WorkerEntrypoint): pass";
  const input = {
    compatibilityDate: "2026-09-08",
    mainModule: "main.py",
    modules: { "main.py": { py: python } },
    env: { KV: root, MESSAGE: "中文" },
    globalOutbound: null,
    limits: { cpuMs: 1000, subRequests: 10 },
  };
  const loader = {
    load(code) {
      return code;
    },
    get(id, callback) {
      return { id, callback };
    },
  };
  const code = loadWorker(loader, input, roots, owner);
  assert.equal(code.mainModule, "main.py");
  assert.deepEqual(
    code.modules,
    Object.assign(Object.create(null), input.modules),
  );
  assert.deepEqual(code.env, { MESSAGE: "中文" });
  assert.deepEqual(code.openComputeBindings.KV, {
    kind: "kvNamespace",
    fetcher: transport,
  });
  assert.equal(code.openComputePrivateEnv.KV, undefined);
  assert.equal(code.compatibilityFlags, undefined);
  assert.equal(code.openComputePythonSnapshot, undefined);
  assert.equal(code.openComputeCache, undefined);
  assert.equal(code.globalOutbound, null);
  assert.deepEqual(code.limits, input.limits);
  let calls = 0;
  const child = getWorker(
    loader,
    "python",
    () => {
      ++calls;
      return input;
    },
    roots,
    "source",
    owner,
  );
  assert.equal(child.id, "source/python");
  assert.equal(calls, 0);
  assert.deepEqual(await child.callback(), code);
  assert.equal(calls, 1);
  const raw = loadWorker(
    loader,
    { ...input, modules: { "main.py": python } },
    roots,
    owner,
  );
  assert.equal(raw.modules["main.py"], python);
  for (const privateField of [
    "openComputePythonSnapshot",
    "openComputePrivateEnv",
    "openComputeBindings",
  ]) {
    assert.throws(
      () => loadWorker(loader, { ...input, [privateField]: {} }, roots, owner),
      /FORWARDING_DENIED/,
    );
  }
});

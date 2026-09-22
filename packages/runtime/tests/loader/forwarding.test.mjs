import assert from "node:assert/strict";
import test from "node:test";
import { parseSync } from "rolldown/utils";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const generator = moduleUrl(
  await compileRuntime("loader/wrappers/generator.ts"),
);
const source = await compileRuntime("loader/forwarding.ts");
const { createForwarding } = await import(moduleUrl(source));
const {
  generateBindingWrapper,
  INTERNAL_MODULE_PREFIX,
  LOADED_ISOLATE_WRAPPER_MODULE,
} = await import(generator);
const { getWorker, loadWorker } = createForwarding(
  generateBindingWrapper,
  INTERNAL_MODULE_PREFIX,
  LOADED_ISOLATE_WRAPPER_MODULE,
  {
    get(id, callback) {
      return this.get(id, callback);
    },
    load(code) {
      return this.load(code);
    },
  },
);

test("tenant array iterator cannot change child wrapper imports", () => {
  const original = Array.prototype[Symbol.iterator];
  const originalToJSON = Array.prototype.toJSON;
  Array.prototype[Symbol.iterator] = function (...args) {
    if (this.length === 3 && this[0] === "kv_namespace")
      return Reflect.apply(
        original,
        ["kv_namespace", "evil.js", "Injected"],
        [],
      );
    return Reflect.apply(original, this, args);
  };
  Array.prototype.toJSON = () => ["EVIL"];
  try {
    const code = generateBindingWrapper({
      mainModule: "main.js",
      bindings: [{ kind: "kv_namespace", name: "KV", capabilityVersion: 1 }],
      services: [],
      durableObject: false,
      automaticCacheEnabled: false,
      cacheFailOpen: false,
      forwardedChild: true,
    });
    assert.match(code, /import \{ KVNamespace \} from "\.\/kv\/facade\.js"/);
    assert.doesNotMatch(code, /evil\.js|Injected/);
    assert.match(code, /names: \["KV"\]/);
    assert.doesNotMatch(code, /EVIL/);
  } finally {
    Array.prototype[Symbol.iterator] = original;
    if (originalToJSON === undefined) delete Array.prototype.toJSON;
    else Array.prototype.toJSON = originalToJSON;
  }
});

function input(value) {
  return {
    compatibilityDate: "2026-09-08",
    mainModule: "main.js",
    modules: { "main.js": { js: "export default { fetch() {} };" } },
    env: { VALUE: 7, KV: value },
  };
}

test("forwarded root uses private transport and platform wrapper without changing native get laziness", async () => {
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
  const sources = {
    "__open_compute__/kv/facade.js": "export class KVNamespace {}",
  };
  let calls = 0;
  const stub = getWorker(
    loader,
    "code-1",
    () => {
      calls++;
      return input(root);
    },
    roots,
    sources,
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
  assert.equal(code.openComputePrivateEnv.KV, transport);
  assert.ok(code.modules[code.mainModule].js.includes('"KV"'));
  assert.doesNotMatch(code.modules[code.mainModule].js, /export \* from/);
  assert.ok(code.modules["__open_compute__/kv/facade.js"]);
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
    {},
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
    {},
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
  loadWorker(loader, input(new FakeBinding()), roots, {}, owner);
  assert.equal(captured.env.KV instanceof FakeBinding, true);
  assert.deepEqual(Object.keys(captured.openComputePrivateEnv), []);
  assert.throws(
    () =>
      loadWorker(
        loader,
        { ...input(root), modules: { "__open_compute__/entry.js": "bad" } },
        roots,
        {},
        owner,
      ),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
  for (const name of [
    "./__open_compute__/entry.js",
    "tenant/../__open_compute__/entry.js",
    "tenant\\..\\__open_compute__\\entry.js",
  ]) {
    assert.throws(
      () =>
        loadWorker(
          loader,
          { ...input(root), modules: { [name]: "bad" } },
          roots,
          {},
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
        {},
        owner,
      ),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
  assert.throws(
    () =>
      loadWorker(
        loader,
        { ...input(root), env: { __KV: root } },
        roots,
        {},
        owner,
      ),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
  assert.throws(
    () => loadWorker({ load: (value) => value }, input(root), roots, {}, {}),
    /WORKER_LOADER_FORWARDING_DENIED/,
  );
});

test("every product root is rewrapped from its private transport", () => {
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
  const code = loadWorker(
    loader,
    { ...input(undefined), env },
    roots,
    {},
    owner,
  );
  assert.deepEqual(Object.keys(code.env), []);
  assert.equal(Object.keys(code.openComputePrivateEnv).length, 14);
  const wrapper = code.modules[code.mainModule].js;
  assert.deepEqual(
    parseSync("entry.js", wrapper, { sourceType: "module" }).errors,
    [],
  );
  for (const name of Object.keys(env)) {
    assert.equal(
      code.openComputePrivateEnv[name],
      roots.get(env[name]).transport,
    );
    assert.ok(wrapper.includes(JSON.stringify(name)), name);
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
  const safe = createForwarding(
    generateBindingWrapper,
    INTERNAL_MODULE_PREFIX,
    LOADED_ISOLATE_WRAPPER_MODULE,
    { get: loaderPrototype.get, load: loaderPrototype.load },
  );
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
    const code = safe.loadWorker(loader, input(root), roots, {}, owner);
    assert.equal(code.env.KV, root);
    assert.equal(code.openComputePrivateEnv.KV, undefined);
  } finally {
    WeakMap.prototype.get = originalWeakGet;
    loaderPrototype.load = originalLoad;
  }
});

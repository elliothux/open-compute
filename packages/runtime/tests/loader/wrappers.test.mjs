import assert from "node:assert/strict";
import test from "node:test";
import { parseSync } from "rolldown/utils";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  import { AsyncLocalStorage } from "node:async_hooks";
  export const scope = new AsyncLocalStorage();
  const exportScope = new AsyncLocalStorage();
  export function withEnv(env, fn) { return scope.run(env, fn); }
  const SocketService = function SocketService() {
    if (this !== exports) throw new Error("entrypoint receiver lost");
    return { specialized: true };
  };
  SocketService.connect = function connect(value) {
    if (this !== SocketService) throw new Error("service receiver lost");
    return value;
  };
  export const workerExports = {
    PublicEntrypoint({ props }) { return { value: props.value }; },
    SocketService,
    __OpenComputeDefaultService() { throw new Error("private default reached"); },
  };
  const activeExports = () => exportScope.getStore() ?? workerExports;
  export const exports = new Proxy(Object.create(null), {
    get(_target, property) { return Reflect.get(activeExports(), property, activeExports()); },
    has(_target, property) { return Reflect.has(activeExports(), property); },
    ownKeys() { return Reflect.ownKeys(activeExports()); },
    getOwnPropertyDescriptor(_target, property) {
      const descriptor = Reflect.getOwnPropertyDescriptor(activeExports(), property);
      return descriptor ? { ...descriptor, configurable: true } : undefined;
    },
  });
  export function withExports(value, fn) { return exportScope.run(value, fn); }
  export const tracing = {};
  export function waitUntil(promise) { promise.catch(() => undefined); }
  export class RpcTarget {}
  export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
  export class WorkflowEntrypoint extends WorkerEntrypoint {}
`);
const cloudflareModule = await import(cloudflare);
const { scope } = cloudflareModule;
const serviceFacade = moduleUrl(`
  export const completions = [];
  export const completeServiceScope = async (_env, scopeId) => { completions.push(scopeId); };
  export const decodeServiceValue = value => value;
  export const encodeServiceValue = value => value;
`);
const serviceScope = moduleUrl(`
  export let scopeRuns = 0;
  export const rootServiceFrame = () => ({ scopeId: crypto.randomUUID(), parentFrame: null });
  export const childServiceFrame = (scopeId, parentFrame) => ({ scopeId, parentFrame });
  export const withServiceScope = (_env, _frame, action) => { scopeRuns += 1; return action(_env); };
`);
const workflowFacade = moduleUrl(`
  export const scheduledCalls = [];
  export async function triggerWorkflowSchedule(binding, schedule) {
    scheduledCalls.push({ binding, schedule });
  }
`);
const runtimeUrl = moduleUrl(await compileRuntime("loader/wrappers/runtime.ts", {
  "cloudflare:workers": cloudflare,
  "../../services/facade.js": serviceFacade,
  "../../services/scope.js": serviceScope,
}));
const {
  createEnvironment, trackExecutionContext, wrapDefault, wrapDefaultService, wrapEntrypoint, validationHandler,
} = await import(runtimeUrl);
const { completions } = await import(serviceFacade);
const serviceScopeState = await import(serviceScope);
const workflowFacadeState = await import(workflowFacade);
const { createWorkflowEntrypoint } = await importRuntime("loader/wrappers/workflow.ts", {
  "cloudflare:workers": cloudflare,
  "./runtime.js": runtimeUrl,
});
const alarmShim = moduleUrl(`
  export const prepareDurableObjectContext = context => ({ context, gate: {} });
  export const activateDurableObjectAlarm = () => {};
  export const dispatchDurableObjectAlarm = () => {};
  export const repairDurableObjectAlarm = () => {};
`);
const outputGate = moduleUrl(`
  export function runWithOutputGate(_gate, fn) { return fn(); }
`);
const facets = moduleUrl(`
  export function prepareTenantFacets(_ctx, _manager, _authority, logicalPath, tenantProps) {
    return { facets: {}, logicalPath: logicalPath ?? [], tenantProps };
  }
`);
const { wrapDurableObject } = await importRuntime("loader/wrappers/durable-object.ts", {
  "../../durable-objects/alarm-shim.js": alarmShim,
  "../../durable-objects/facets.js": facets,
  "../../durable-objects/output-gate.js": outputGate,
  "./runtime.js": runtimeUrl,
});
const generator = await importRuntime("loader/wrappers/generator.ts");

test("env capabilities wrap once and remove every private host capability", () => {
  const calls = [];
  class Capability { constructor(raw, durableObject) { calls.push([raw, durableObject]); this.raw = raw; } }
  const wrap = createEnvironment([
    { names: ["DB"], create: Capability },
    { names: ["OBJECTS"], create: Capability },
  ], true);
  const env = JSON.parse('{"DB":"raw","OBJECTS":"private-raw","__proto__":{"safe":true},"__OPEN_COMPUTE_PRIVATE_ALARM_INDEX":"private","__OPEN_COMPUTE_PRIVATE_CACHE":"cache"}');
  const wrapped = cloudflareModule.withExports(
    { PublicEntrypoint: cloudflareModule.workerExports.PublicEntrypoint },
    () => wrap(env),
  );
  assert.equal(Object.getPrototypeOf(wrapped), Object.prototype);
  assert.deepEqual(Object.getOwnPropertyDescriptor(wrapped, "__proto__").value, { safe: true });
  assert.deepEqual(calls, [["raw", true], ["private-raw", true]]);
  assert.equal(wrapped.OBJECTS.raw, "private-raw");
  assert.equal(wrap(wrapped), wrapped);
  assert.equal(calls.length, 2);
  assert.equal("__OPEN_COMPUTE_PRIVATE_ALARM_INDEX" in wrapped, false);
  assert.equal("__OPEN_COMPUTE_PRIVATE_CACHE" in wrapped, false);
  assert.equal(env.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, "private");
  assert.equal(env.__OPEN_COMPUTE_PRIVATE_CACHE, "cache");
});

test("tenant ctx.exports and importable exports expose no private generated entrypoints", async () => {
  const native = { waitUntil(promise) { promise.catch(() => undefined); } };
  const context = trackExecutionContext(native).context;
  const exported = context.exports;
  assert.deepEqual(exported.PublicEntrypoint({ props: { value: 42 } }), { value: 42 });
  assert.deepEqual(exported.SocketService(), { specialized: true });
  const connect = exported.SocketService.connect;
  assert.equal(connect("connected"), "connected");
  assert.equal(exported.SocketService.connect, connect);
  for (const name of ["__OpenComputeDefaultService"]) {
    assert.equal(exported[name], undefined);
    assert.equal(name in exported, false);
    assert.equal(Object.getOwnPropertyDescriptor(exported, name), undefined);
  }
  assert.deepEqual(Reflect.ownKeys(exported), ["PublicEntrypoint", "SocketService"]);
  assert.deepEqual(Object.keys(exported), ["PublicEntrypoint", "SocketService"]);
  assert.equal(Object.getPrototypeOf(exported), null);
  assert.equal(Object.getPrototypeOf(context), null);
  assert.equal(Object.getOwnPropertyDescriptor(context, "exports").value, exported);
  const wrapped = wrapDefault({
    fetch() {
      assert.equal(cloudflareModule.exports.__OpenComputeDefaultService, undefined);
      assert.deepEqual(
        Reflect.ownKeys(cloudflareModule.exports),
        ["PublicEntrypoint", "SocketService"],
      );
      return new Response("safe");
    },
  }, createEnvironment([], false));
  assert.equal(await (await wrapped.fetch(
    new Request("https://example.invalid/"),
    {},
    native,
  )).text(), "safe");
});

test("object and function handlers restore async env scope and preserve event receivers", async () => {
  const wrap = createEnvironment([], false);
  const handler = {
    label: "owner",
    async fetch(_request, env) {
      await Promise.resolve();
      assert.equal(scope.getStore(), env);
      assert.equal(env.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, undefined);
      return this.label;
    },
    scheduled(event) { assert.equal(event.type, "scheduled"); return event.read(); },
    trace(event, env, ctx) {
      assert.equal(this, handler);
      assert.equal(env.TRACE, "ok");
      assert.equal(typeof ctx.waitUntil, "function");
      return event.read();
    },
  };
  const wrapped = wrapDefault(handler, wrap);
  const context = { waitUntil(promise) { promise.catch(() => undefined); } };
  assert.equal(await wrapped.fetch(new Request("https://example.invalid"), { TOKEN: "value", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "private" }, context), "owner");
  class Event { #value = 42; read() { return this.#value; } }
  assert.equal(wrapped.scheduled(new Event(), {}, context), 42);
  assert.equal(wrapped.trace(new Event(), { TRACE: "ok" }, context), 42);
  const fn = wrapDefault((_event, env) => env.MESSAGE, wrap);
  assert.equal(fn.fetch({}, { MESSAGE: "ok" }, context), "ok");
  assert.equal(scope.getStore(), undefined);
});

test("direct Workflow schedules run before the optional tenant handler and hide trusted targets", async () => {
  const start = workflowFacadeState.scheduledCalls.length;
  const flow = { binding: "FLOW" };
  const context = { waitUntil(promise) { promise.catch(() => undefined); } };
  let invoked = 0;
  const handler = {
    async scheduled(controller, env, ctx) {
      invoked += 1;
      assert.equal(this, handler);
      assert.equal(env.FLOW, flow);
      assert.equal(ctx.waitUntil instanceof Function, true);
      assert.equal(controller.type, "scheduled");
      assert.equal(controller.cron, "*/5 * * * *");
      assert.equal(controller.scheduledTime, 1_788_048_000_000);
      assert.equal(controller.scheduledHandler, undefined);
      assert.equal(controller.workflowBindings, undefined);
      assert.equal("scheduledHandler" in controller, false);
      assert.equal("workflowBindings" in controller, false);
      assert.equal(Reflect.ownKeys(controller).includes("workflowBindings"), false);
      controller.noRetry();
      return "tenant-result";
    },
  };
  const wrapped = wrapDefault(
    handler,
    createEnvironment([], false),
    undefined,
    {
      targets: [
        { cron: "*/5 * * * *", scheduledHandler: true, workflowBindings: ["FLOW"] },
        { cron: "0 * * * *", scheduledHandler: false, workflowBindings: ["FLOW"] },
      ],
      trigger: workflowFacadeState.triggerWorkflowSchedule,
    },
  );
  let noRetry = 0;
  const event = {
    scheduledTime: 1_788_048_000_000,
    cron: "*/5 * * * *",
    noRetry() { noRetry += 1; },
  };
  assert.equal(await wrapped.scheduled(event, { FLOW: flow }, context), "tenant-result");
  assert.equal(invoked, 1);
  assert.equal(noRetry, 1);
  assert.deepEqual(workflowFacadeState.scheduledCalls.slice(start), [{
    binding: flow,
    schedule: { cron: "*/5 * * * *", scheduledTime: 1_788_048_000_000 },
  }]);

  const workflowOnly = {
    ...event,
    cron: "0 * * * *",
  };
  assert.equal(await wrapped.scheduled(workflowOnly, { FLOW: flow }, context), undefined);
  assert.equal(invoked, 1);
  assert.equal(workflowFacadeState.scheduledCalls.length, start + 2);
  const invalid = wrapDefault(handler, createEnvironment([], false), undefined, {
    targets: [{ cron: "*/5 * * * *", scheduledHandler: true, workflowBindings: ["FLOW", "FLOW"] }],
    trigger: workflowFacadeState.triggerWorkflowSchedule,
  });
  await assert.rejects(invalid.scheduled(event, { FLOW: flow }, context), /CRON_CUSTOM_EVENT_UNSUPPORTED/);
});

test("class construction, async RPC and private fields keep their native receivers and env", async () => {
  let constructed;
  class Tenant {
    #value;
    constructor(ctx, env) { assert.equal(scope.getStore(), env); constructed = env; this.#value = ctx; }
    async read() { await Promise.resolve(); assert.equal(scope.getStore(), constructed); return this.#value; }
  }
  const Wrapped = wrapEntrypoint(Tenant, createEnvironment([], false), "Named");
  const context = { value: 42, waitUntil(promise) { promise.catch(() => undefined); } };
  const instance = new Wrapped(context, { TOKEN: "value", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "private" });
  assert.equal(Wrapped.name, "Named");
  assert.ok(instance instanceof Tenant);
  assert.equal((await instance.read()).value, 42);
  assert.equal(constructed.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, undefined);
  assert.equal(scope.getStore(), undefined);
  const Default = wrapDefault(Tenant, createEnvironment([], false));
  assert.equal((await new Default({ ...context, value: 43 }, {}).read()).value, 43);
  for (const invalid of [null, {}, () => {}]) assert.throws(() => wrapEntrypoint(invalid, value => value), /missing entrypoint/);
});

test("Service dispatch returns values before its native background completion stream drains", async () => {
  let finish;
  const background = new Promise(resolve => { finish = resolve; });
  class Tenant {
    constructor(ctx) { this.ctx = ctx; }
    operation() { this.ctx.waitUntil(background); return 42; }
  }
  const context = { waitUntil(promise) { promise.catch(() => undefined); } };
  const instance = new (wrapEntrypoint(Tenant, createEnvironment([], false)))(context, {});
  const reporter = {
    beginCapability() {}, releaseRetention() {}, completeOperation() {},
    retainCapability() {}, dup() { return this; }, [Symbol.dispose]() {},
  };
  const envelope = await instance.__openComputeServiceRpc(
    crypto.randomUUID(), crypto.randomUUID(), reporter, "operation", [],
  );
  assert.equal(envelope.ok, true);
  assert.equal(envelope.value, 42);
  const reader = envelope.background.getReader();
  let drained = false;
  const read = reader.read().then(part => { drained = part.done; });
  await Promise.resolve();
  assert.equal(drained, false);
  finish();
  await read;
  assert.equal(drained, true);
});

test("object and function default Service fetches receive the target env and context", async () => {
  const reporter = {
    beginCapability() {}, releaseRetention() {}, completeOperation() {},
    retainCapability() {}, dup() { return this; }, [Symbol.dispose]() {},
  };
  const context = { waitUntil(promise) { promise.catch(() => undefined); } };
  const object = {
    fetch(request, env, ctx) {
      ctx.waitUntil(Promise.resolve());
      return new Response(`${this === object}:${env.OWNER}:${new URL(request.url).hostname}`);
    },
  };
  for (const [raw, expected] of [
    [object, "true:object:service.example"],
    [(_request, env) => new Response(`function:${env.OWNER}`), "function:function"],
  ]) {
    const DefaultService = wrapDefaultService(raw, createEnvironment([], false));
    const instance = new DefaultService(context, { OWNER: expected.startsWith("true") ? "object" : "function" });
    const envelope = await instance.__openComputeServiceFetch(
      crypto.randomUUID(), crypto.randomUUID(), reporter,
      new Request("https://service.example/path"),
    );
    assert.equal(envelope.ok, true);
    assert.equal(await envelope.value.text(), expected);
    assert.equal((await envelope.background.getReader().read()).done, true);
  }
});

test("object default Service connect receives the native socket, target env, and context", async () => {
  const context = { marker: "ctx", waitUntil(promise) { promise.catch(() => undefined); } };
  const socket = { native: true };
  const object = {
    async connect(actual, env, ctx) {
      await Promise.resolve();
      assert.equal(this, object);
      assert.equal(actual, socket);
      assert.equal(env.OWNER, "object");
      assert.equal(ctx.marker, "ctx");
      assert.equal(scope.getStore(), env);
    },
  };
  const DefaultService = wrapDefaultService(object, createEnvironment([], false));
  await new DefaultService(context, { OWNER: "object" }).connect(socket);
  assert.equal(scope.getStore(), undefined);
});

test("Workflow entrypoints give the private controller only to the runner", async () => {
  const priorScopes = serviceScopeState.scopeRuns;
  const priorCompletions = completions.length;
  const controller = { privateGrant: "private" };
  const target = class {};
  const context = {
    context: true,
    waitUntil(promise) { promise.catch(() => undefined); },
  };
  const Entry = createWorkflowEntrypoint(target, createEnvironment([], false), async (actual, ctx, env, event, backend) => {
    assert.equal(actual, target);
    assert.equal(ctx, context);
    assert.equal(ctx.context, true);
    assert.equal(scope.getStore(), env);
    assert.equal(backend, controller);
    assert.deepEqual(env, { USER: "public" });
    assert.deepEqual(event, { payloadJson: "null" });
    return { outcome: "complete", outputJson: "42", finalOrdinal: 0 };
  }, value => value === target);
  const entry = new Entry(context, { USER: "public", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "hidden" });
  assert.equal(entry.ctx, context);
  assert.equal(entry.validate(), true);
  assert.equal((await entry.execute({ payloadJson: "null" }, controller)).outcome, "complete");
  for (let attempt = 0; attempt < 10 && completions.length === priorCompletions; attempt += 1) {
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(serviceScopeState.scopeRuns, priorScopes + 1);
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(scope.getStore(), undefined);
});

test("Durable Object methods share the root Service scope and tracked waitUntil lifecycle", async () => {
  const priorScopes = serviceScopeState.scopeRuns;
  const priorCompletions = completions.length;
  class Capability { constructor(raw) { this.value = raw; } }
  class Tenant {
    constructor(ctx, env) {
      this.ctx = ctx;
      this.env = env;
      this.privateExportsHidden = ctx.exports.__OpenComputeDefaultService === undefined;
    }
    async fetch() {
      this.ctx.waitUntil(Promise.resolve());
      return `${this.env.VALUE}:${this.ctx.storage}:${this.env.OBJECTS.value}:${this.privateExportsHidden}`;
    }
  }
  const Wrapped = wrapDurableObject(Tenant, createEnvironment([{
    names: ["OBJECTS"], create: Capability,
  }], true), "Object");
  let contextWaits = 0;
  const context = {
    get storage() {
      if (this !== context) throw new TypeError("invalid native context receiver");
      return "native";
    },
    waitUntil(promise) { contextWaits += 1; promise.catch(() => undefined); },
    exports: cloudflareModule.workerExports,
  };
  const index = { upsert() {}, delete() {}, clear() {} };
  const facetManager = { __openComputeFacetCall() {}, __openComputeFacetClone() {} };
  const facetAuthority = {
    accountId: "account",
    workerId: "worker",
    deploymentId: "deployment",
    workerCodeSha256: "a".repeat(64),
    className: "Object",
  };
  const instance = cloudflareModule.withExports(
    { PublicEntrypoint: cloudflareModule.workerExports.PublicEntrypoint },
    () => new Wrapped(context, {
      VALUE: "ok", OBJECTS: "trusted", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: index,
      __OPEN_COMPUTE_PRIVATE_FACET_MANAGER: facetManager,
      __OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY: facetAuthority,
      __OPEN_COMPUTE_PRIVATE_FACET_PATH: [],
      __OPEN_COMPUTE_PRIVATE_FACET_PROPS: undefined,
    }),
  );
  assert.equal(await instance.fetch(), "ok:native:trusted:true");
  assert.equal(context.exports.__OpenComputeDefaultService, undefined);
  assert.equal(Object.getOwnPropertyDescriptor(context, "exports").configurable, false);
  for (let attempt = 0; attempt < 10 && completions.length === priorCompletions; attempt += 1) {
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(serviceScopeState.scopeRuns, priorScopes + 1);
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(contextWaits, 2);
});

test("Durable Object WebSocket responses hand ownership to native hibernation", async () => {
  const priorCompletions = completions.length;
  class Tenant {
    fetch() {
      const response = new Response(null, { status: 200 });
      Object.defineProperty(response, "webSocket", { value: new EventTarget() });
      return response;
    }
  }
  const Wrapped = wrapDurableObject(Tenant, createEnvironment([], true), "SocketObject");
  const waits = [];
  const instance = new Wrapped({
    storage: "native",
    exports: cloudflareModule.workerExports,
    waitUntil(promise) { waits.push(promise); promise.catch(() => undefined); },
  }, {
    __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: { upsert() {}, delete() {}, clear() {} },
    __OPEN_COMPUTE_PRIVATE_FACET_MANAGER: { __openComputeFacetCall() {}, __openComputeFacetClone() {} },
    __OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY: {
      accountId: "account",
      workerId: "worker",
      deploymentId: "deployment",
      workerCodeSha256: "a".repeat(64),
      className: "SocketObject",
    },
    __OPEN_COMPUTE_PRIVATE_FACET_PATH: [],
    __OPEN_COMPUTE_PRIVATE_FACET_PROPS: undefined,
  });
  const response = instance.fetch();
  assert.ok(response.webSocket instanceof EventTarget);
  for (let attempt = 0; attempt < 10 && completions.length === priorCompletions; attempt += 1) {
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(waits.length, 1);
});

test("generated modules only wire imports and configuration into the checked runtime", async () => {
  const tenant = moduleUrl(`export const named = 42; export default { fetch(_request, env) { return env.GREETING; } };`);
  const code = generator.generateBindingWrapper({ mainModule: "index.js", bindings: [], services: [], durableObject: false });
  assert.deepEqual(parseSync("entry.js", code, { sourceType: "module" }).errors, []);
  assert.doesNotMatch(code, /\b(class|function|for|if)\b/);
  const mapped = code.replaceAll('"../index.js"', JSON.stringify(tenant))
    .replaceAll('"./loader/wrappers/runtime.js"', JSON.stringify(runtimeUrl));
  const entry = await import(moduleUrl(mapped));
  assert.equal(entry.named, 42);
  assert.equal(entry.default.fetch({}, { GREETING: "hello" }, {
    waitUntil(promise) { promise.catch(() => undefined); },
  }), "hello");
  assert.equal(await validationHandler(entry, "default").fetch().text(), "open-compute-validation-v1");
  assert.throws(() => validationHandler(entry, "missing"), /missing entrypoint/);
  assert.equal(generator.generateBindingWrapper({ mainModule: "index.js", bindings: [], services: [], entrypointName: "default", durableObject: false }), code);
  for (const name of ['bad";throw 1;', "nested.name", "A".repeat(129)]) {
    assert.throws(() => generator.generateBindingWrapper({ mainModule: "index.js", bindings: [], services: [], entrypointName: name, durableObject: false }), /invalid entrypoint/);
  }
});

test("all binding and entrypoint combinations produce valid import-only bridges", () => {
  const bindings = [
    ["r2_bucket", 1, "BUCKET"], ["d1_database", 1, "DATABASE"], ["do_namespace", 1, "OBJECTS"],
    ["queue_producer", 1, "QUEUE"], ["workflow", 1, "FLOW"],
    ["vectorize_index", 1, "VECTOR"], ["ai_search_namespace", 1, "SEARCH_NS"],
    ["ai_search_instance", 1, "SEARCH"],
  ].map(([kind, capabilityVersion, name]) => ({ kind, capabilityVersion, name }));
  for (const options of [{}, { entrypointName: "default" }, { entrypointName: "Named" }, { entrypointName: "Object", durableObject: true }, { entrypointName: "default", durableObject: true },
    { entrypointName: "Flow", workflow: true }]) {
    const code = generator.generateBindingWrapper({
      mainModule: "src/index.js", bindings,
      services: [{ name: "CATALOG" }], assetBindingName: "ASSETS", imagesBindingName: "IMAGES",
      aiBindingName: "AI", durableObject: false, ...options,
    });
    assert.deepEqual(parseSync("entry.js", code, { sourceType: "module" }).errors, []);
    assert.match(code, /WorkflowBinding/);
    assert.match(code, /AssetsBinding/);
    assert.match(code, /ServiceBinding/);
    assert.match(code, /ImagesBinding/);
    assert.match(code, /AiBinding/);
    assert.match(code, /VectorizeBinding/);
    assert.match(code, /AiSearchNamespaceBinding/);
    assert.match(code, /AiSearchInstanceBinding/);
    assert.doesNotMatch(code, /internalExport|DurableObjectStubTransport|__OpenComputeDoStubTransport/);
    assert.doesNotMatch(code, /\b(class|function|for|if)\b/);
  }
  assert.deepEqual(parseSync("validation.js", generator.generateValidationWrapper("Named"), { sourceType: "module" }).errors, []);
});

test("default bridge wraps every enabled ctx.exports cache entrypoint", () => {
  const code = generator.generateBindingWrapper({
    mainModule: "index.js", bindings: [], services: [], durableObject: false,
    cacheAvailable: true, automaticCacheEnabled: true, cacheFailOpen: true,
    automaticCacheEntrypoints: ["Named", "Api"],
  });
  assert.deepEqual(parseSync("entry.js", code, { sourceType: "module" }).errors, []);
  assert.match(code, /createCacheRuntime\(true, true, "default"\)/);
  assert.match(code, /createCacheRuntime\(true, true, "Named"\)/);
  assert.match(code, /createCacheRuntime\(true, true, "Api"\)/);
  assert.match(code, /as Named/);
  assert.match(code, /as Api/);
});

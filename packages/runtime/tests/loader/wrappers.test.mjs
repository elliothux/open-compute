import assert from "node:assert/strict";
import test from "node:test";
import { parseSync } from "rolldown/utils";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  import { AsyncLocalStorage } from "node:async_hooks";
  export const scope = new AsyncLocalStorage();
  export function withEnv(env, fn) { return scope.run(env, fn); }
  export const exports = {};
  export const tracing = {};
  export function waitUntil(promise) { promise.catch(() => undefined); }
  export class RpcTarget {}
  export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
  export class WorkflowEntrypoint extends WorkerEntrypoint {}
`);
const { scope } = await import(cloudflare);
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
const runtimeUrl = moduleUrl(await compileRuntime("loader/wrappers/runtime.ts", {
  "cloudflare:workers": cloudflare,
  "../../services/facade.js": serviceFacade,
  "../../services/scope.js": serviceScope,
}));
const {
  createEnvironment, wrapDefault, wrapDefaultService, wrapEntrypoint, validationHandler,
} = await import(runtimeUrl);
const { completions } = await import(serviceFacade);
const serviceScopeState = await import(serviceScope);
const { createWorkflowEntrypoint } = await importRuntime("loader/wrappers/workflow.ts", {
  "cloudflare:workers": cloudflare,
  "./runtime.js": runtimeUrl,
});
const alarmShim = moduleUrl(`
  export const prepareDurableObjectContext = context => ({ context });
  export const activateDurableObjectAlarm = () => {};
  export const dispatchDurableObjectAlarm = () => {};
  export const repairDurableObjectAlarm = () => {};
`);
const { wrapDurableObject } = await importRuntime("loader/wrappers/durable-object.ts", {
  "../../durable-objects/alarm-shim.js": alarmShim,
  "./runtime.js": runtimeUrl,
});
const generator = await importRuntime("loader/wrappers/generator.ts");

test("env capabilities wrap once, preserve JSON keys, and never expose the private alarm index", () => {
  const calls = [];
  class Capability { constructor(raw, durableObject) { calls.push([raw, durableObject]); this.raw = raw; } }
  const wrap = createEnvironment([{ names: ["DB"], create: Capability }], true);
  const env = JSON.parse('{"DB":"raw","__proto__":{"safe":true},"__OPEN_COMPUTE_PRIVATE_ALARM_INDEX":"private"}');
  const wrapped = wrap(env);
  assert.equal(Object.getPrototypeOf(wrapped), Object.prototype);
  assert.deepEqual(Object.getOwnPropertyDescriptor(wrapped, "__proto__").value, { safe: true });
  assert.deepEqual(calls, [["raw", true]]);
  assert.equal(wrap(wrapped), wrapped);
  assert.equal(calls.length, 1);
  assert.equal("__OPEN_COMPUTE_PRIVATE_ALARM_INDEX" in wrapped, false);
  assert.equal(env.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, "private");
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
  };
  const wrapped = wrapDefault(handler, wrap);
  const context = { waitUntil(promise) { promise.catch(() => undefined); } };
  assert.equal(await wrapped.fetch(new Request("https://example.invalid"), { TOKEN: "value", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "private" }, context), "owner");
  class Event { #value = 42; read() { return this.#value; } }
  assert.equal(wrapped.scheduled(new Event(), {}, context), 42);
  const fn = wrapDefault((_event, env) => env.MESSAGE, wrap);
  assert.equal(fn.fetch({}, { MESSAGE: "ok" }, context), "ok");
  assert.equal(scope.getStore(), undefined);
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
  class Tenant {
    constructor(ctx, env) { this.ctx = ctx; this.env = env; }
    async fetch() {
      this.ctx.waitUntil(Promise.resolve());
      return `${this.env.VALUE}:${this.ctx.storage}`;
    }
  }
  const Wrapped = wrapDurableObject(Tenant, createEnvironment([], true), "Object");
  let contextWaits = 0;
  const context = {
    get storage() {
      if (this !== context) throw new TypeError("invalid native context receiver");
      return "native";
    },
    waitUntil(promise) { contextWaits += 1; promise.catch(() => undefined); },
  };
  const index = { upsert() {}, delete() {}, clear() {} };
  const instance = new Wrapped(context, {
    VALUE: "ok", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: index,
  });
  assert.equal(await instance.fetch(), "ok:native");
  for (let attempt = 0; attempt < 10 && completions.length === priorCompletions; attempt += 1) {
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(serviceScopeState.scopeRuns, priorScopes + 1);
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(contextWaits, 2);
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
  ].map(([kind, capabilityVersion, name]) => ({ kind, capabilityVersion, name }));
  for (const options of [{}, { entrypointName: "default" }, { entrypointName: "Named" }, { entrypointName: "Object", durableObject: true }, { entrypointName: "default", durableObject: true },
    { entrypointName: "Flow", workflow: true }]) {
    const code = generator.generateBindingWrapper({
      mainModule: "src/index.js", bindings,
      services: [{ name: "CATALOG" }], assetBindingName: "ASSETS", durableObject: false, ...options,
    });
    assert.deepEqual(parseSync("entry.js", code, { sourceType: "module" }).errors, []);
    assert.match(code, /WorkflowBinding/);
    assert.match(code, /AssetsBinding/);
    assert.match(code, /ServiceBinding/);
    assert.doesNotMatch(code, /\b(class|function|for|if)\b/);
  }
  assert.deepEqual(parseSync("validation.js", generator.generateValidationWrapper("Named"), { sourceType: "module" }).errors, []);
});

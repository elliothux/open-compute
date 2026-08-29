import assert from "node:assert/strict";
import test from "node:test";
import { parseSync } from "rolldown/utils";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  import { AsyncLocalStorage } from "node:async_hooks";
  export const scope = new AsyncLocalStorage();
  export function withEnv(env, fn) { return scope.run(env, fn); }
  export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
  export class WorkflowEntrypoint extends WorkerEntrypoint {}
`);
const { scope } = await import(cloudflare);
const runtimeUrl = moduleUrl(await compileRuntime("loader/wrappers/runtime.ts", { "cloudflare:workers": cloudflare }));
const { createEnvironment, wrapDefault, wrapEntrypoint, validationHandler } = await import(runtimeUrl);
const { createWorkflowEntrypoint } = await importRuntime("loader/wrappers/workflow.ts", { "cloudflare:workers": cloudflare });
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
  assert.equal(await wrapped.fetch(new Request("https://example.invalid"), { TOKEN: "value", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "private" }, {}), "owner");
  class Event { #value = 42; read() { return this.#value; } }
  assert.equal(wrapped.scheduled(new Event(), {}, {}), 42);
  const fn = wrapDefault((_event, env) => env.MESSAGE, wrap);
  assert.equal(fn.fetch({}, { MESSAGE: "ok" }, {}), "ok");
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
  const instance = new Wrapped(42, { TOKEN: "value", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "private" });
  assert.equal(Wrapped.name, "Named");
  assert.ok(instance instanceof Tenant);
  assert.equal(await instance.read(), 42);
  assert.equal(constructed.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, undefined);
  assert.equal(scope.getStore(), undefined);
  const Default = wrapDefault(Tenant, createEnvironment([], false));
  assert.equal(await new Default(43, {}).read(), 43);
  for (const invalid of [null, {}, () => {}]) assert.throws(() => wrapEntrypoint(invalid, value => value), /missing entrypoint/);
});

test("Workflow entrypoints give the private controller only to the runner", async () => {
  const controller = { privateGrant: "private" };
  const target = class {};
  const Entry = createWorkflowEntrypoint(target, createEnvironment([], false), async (actual, ctx, env, event, backend) => {
    assert.equal(actual, target);
    assert.deepEqual(ctx, { context: true });
    assert.equal(scope.getStore(), env);
    assert.equal(backend, controller);
    assert.deepEqual(env, { USER: "public" });
    assert.deepEqual(event, { payloadJson: "null" });
    return { outcome: "complete", outputJson: "42", finalOrdinal: 0 };
  }, value => value === target);
  const entry = new Entry({ context: true }, { USER: "public", __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: "hidden" });
  assert.equal(entry.validate(), true);
  assert.equal((await entry.execute({ payloadJson: "null" }, controller)).outcome, "complete");
  assert.equal(scope.getStore(), undefined);
});

test("generated modules only wire imports and configuration into the checked runtime", async () => {
  const tenant = moduleUrl(`export const named = 42; export default { fetch(_request, env) { return env.GREETING; } };`);
  const code = generator.generateBindingWrapper({ mainModule: "index.js", bindings: [], durableObject: false });
  assert.deepEqual(parseSync("entry.js", code, { sourceType: "module" }).errors, []);
  assert.doesNotMatch(code, /\b(class|function|for|if)\b/);
  const mapped = code.replaceAll('"../index.js"', JSON.stringify(tenant))
    .replaceAll('"./loader/wrappers/runtime.js"', JSON.stringify(runtimeUrl));
  const entry = await import(moduleUrl(mapped));
  assert.equal(entry.named, 42);
  assert.equal(entry.default.fetch({}, { GREETING: "hello" }, {}), "hello");
  assert.equal(await validationHandler(entry, "default").fetch().text(), "open-compute-validation-v1");
  assert.throws(() => validationHandler(entry, "missing"), /missing entrypoint/);
  assert.equal(generator.generateBindingWrapper({ mainModule: "index.js", bindings: [], entrypointName: "default", durableObject: false }), code);
  for (const name of ['bad";throw 1;', "nested.name", "A".repeat(129)]) {
    assert.throws(() => generator.generateBindingWrapper({ mainModule: "index.js", bindings: [], entrypointName: name, durableObject: false }), /invalid entrypoint/);
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
      mainModule: "src/index.js", bindings, assetBindingName: "ASSETS", durableObject: false, ...options,
    });
    assert.deepEqual(parseSync("entry.js", code, { sourceType: "module" }).errors, []);
    assert.match(code, /WorkflowBinding/);
    assert.match(code, /AssetsBinding/);
    assert.doesNotMatch(code, /\b(class|function|for|if)\b/);
  }
  assert.deepEqual(parseSync("validation.js", generator.generateValidationWrapper("Named"), { sourceType: "module" }).errors, []);
});

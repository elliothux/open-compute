import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  import { AsyncLocalStorage } from "node:async_hooks";
  export const scope = new AsyncLocalStorage();
  export const env = new Proxy({}, {
    get(_target, property) { return scope.getStore()?.[property]; },
  });
  export const waitUntilObservers = new AsyncLocalStorage();
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
  export function waitUntil(promise) {
    waitUntilObservers.getStore()?.(promise);
    promise.catch(() => undefined);
  }
  export class RpcTarget {}
  export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
  export class WorkflowEntrypoint extends WorkerEntrypoint {}
`);
const cloudflareModule = await import(cloudflare);
const { scope } = cloudflareModule;
const nativeWorkers = moduleUrl(`
  import { waitUntilObservers } from ${JSON.stringify(cloudflare)};
  export default { withWaitUntilObserver(observer, fn) {
    return waitUntilObservers.run(observer, fn);
  } };
`);
const wrapped = moduleUrl(`
  export default { createServiceRpcStub: value => value };
`);
const serviceFacade = moduleUrl(`
  export const completions = [];
  export const completeServiceScope = async (_env, frame) => { completions.push(frame.scopeId); };
  export const decodeServiceValue = value => value;
  export const encodeServiceValue = value => value;
  export const serviceCapabilityController = (reporter, createStub) => ({ ...reporter, createStub });
  export const attachServiceWebSocketHandoffs = value => value;
`);
const actualServiceScope = moduleUrl(
  await compileRuntime("services/scope.ts", {
    "cloudflare:workers": cloudflare,
  }),
);
const serviceScope = moduleUrl(`
  import * as actual from ${JSON.stringify(actualServiceScope)};
  export { rootServiceFrame, childServiceFrame, currentServiceFrame } from ${JSON.stringify(actualServiceScope)};
  export let scopeRuns = 0;
  export const withServiceScope = (env, frame, action) => {
    scopeRuns += 1;
    return actual.withServiceScope(env, frame, action);
  };
`);
const workflowFacade = moduleUrl(`
  export const scheduledCalls = [];
  export async function triggerWorkflowSchedule(binding, schedule) {
    scheduledCalls.push({ binding, schedule });
  }
`);
const loopbackUrl = moduleUrl(
  await compileRuntime("loader/wrappers/loopback.ts", {
    "cloudflare:workers": cloudflare,
  }),
);
const loopbackModule = await import(loopbackUrl);
const completionUrl = moduleUrl(
  await compileRuntime("loader/wrappers/completion.ts", {
    "../../services/facade.js": serviceFacade,
  }),
);
const runtimeUrl = moduleUrl(
  await compileRuntime("loader/wrappers/runtime.ts", {
    "cloudflare-internal:workers": nativeWorkers,
    "cloudflare-internal:wrapped-binding": wrapped,
    "cloudflare:workers": cloudflare,
    "./completion.js": completionUrl,
    "./loopback.js": loopbackUrl,
    "../../services/capabilities.js": serviceFacade,
    "../../services/rpc-member.js": moduleUrl(
      await compileRuntime("services/rpc-member.ts"),
    ),
    "../../services/scope.js": serviceScope,
  }),
);
const {
  validationHandler,
  trackExecutionContext,
  wrapDefault,
  wrapDefaultService,
  wrapEntrypoint,
} = await import(runtimeUrl);
const { completions } = await import(serviceFacade);
const serviceScopeState = await import(serviceScope);
const workflowFacadeState = await import(workflowFacade);
const { createWorkflowEntrypoint } = await importRuntime(
  "loader/wrappers/workflow.ts",
  {
    "cloudflare:workers": cloudflare,
    "./runtime.js": runtimeUrl,
  },
);
const alarmShim = moduleUrl(`
  export const prepareDurableObjectContext = context => ({ context, gate: {} });
  export const activateDurableObjectAlarm = () => {};
  export const alarmCalls = [];
  export const dispatchDurableObjectAlarm = (owner, handler, state, payload) => {
    alarmCalls.push({ state, payload });
    return Reflect.apply(handler, owner, []);
  };
  export const repairDurableObjectAlarm = state => { alarmCalls.push({ repair: state }); };
`);
const alarmShimState = await import(alarmShim);
const outputGate = moduleUrl(`
  export function runWithOutputGate(_gate, fn) { return fn(); }
`);
const facets = moduleUrl(`
  export function prepareTenantFacets(_ctx, _manager, _authority, logicalPath, tenantProps) {
    return { facets: {}, logicalPath: logicalPath ?? [], tenantProps };
  }
`);
const { wrapDurableObject } = await importRuntime(
  "loader/wrappers/durable-object.ts",
  {
    "../../durable-objects/alarm-shim.js": alarmShim,
    "../../durable-objects/facets.js": facets,
    "../../durable-objects/output-gate.js": outputGate,
    "../../durable-objects/fetch-admission.js": moduleUrl(
      await compileRuntime("durable-objects/fetch-admission.ts"),
    ),
    "../../durable-objects/host-protocol.js": moduleUrl(
      await compileRuntime("durable-objects/host-protocol.ts", {
        "./errors.js": moduleUrl(
          await compileRuntime("durable-objects/errors.ts"),
        ),
        "./identity.js": moduleUrl(
          await compileRuntime("durable-objects/identity.ts"),
        ),
        "../loader/shared.js": moduleUrl(
          "export const bindingError = code => new Error(code);",
        ),
      }),
    ),
    "../../services/rpc-member.js": moduleUrl(
      await compileRuntime("services/rpc-member.ts"),
    ),
    "../../services/facade.js": serviceFacade,
    "../../services/scope.js": serviceScope,
    "./runtime.js": runtimeUrl,
  },
);

test("context waitUntil overrides retain borrowed tasks without mutating the native receiver", async () => {
  const registered = [];
  const native = {
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      assert.equal(this, native);
      registered.push(promise);
      promise.catch(() => undefined);
    },
  };
  const nativeWaitUntil = native.waitUntil;
  const tracked = trackExecutionContext(native);
  const context = tracked.context;
  const original = context.waitUntil;
  assert.equal(context.waitUntil, original);
  assert.equal(
    Object.getOwnPropertyDescriptor(context, "waitUntil").value,
    original,
  );
  let finish;
  let copied = 0;
  let destroyed = 0;
  const retained = new Promise((resolve) => {
    finish = resolve;
  });
  retained.destroy = () => {
    destroyed += 1;
  };
  const borrowed = {
    copy() {
      copied += 1;
      return retained;
    },
    then() {
      throw new Error("borrowed proxy destroyed");
    },
  };
  const patched = (promise) =>
    original(
      (async () => {
        if ("copy" in promise) promise = promise.copy();
        try {
          await promise;
        } finally {
          if ("destroy" in promise) promise.destroy();
        }
      })(),
    );
  context.waitUntil = patched;
  assert.equal(context.waitUntil, patched);
  assert.equal(
    Object.getOwnPropertyDescriptor(context, "waitUntil").value,
    patched,
  );
  assert.equal(
    Reflect.ownKeys(context).filter((key) => key === "waitUntil").length,
    1,
  );
  cloudflareModule.waitUntilObservers.run(
    (promise) => tracked.tasks.push(promise),
    () => context.waitUntil(borrowed),
  );
  assert.equal(copied, 1);
  assert.equal(tracked.tasks.length, 1);
  assert.equal(registered[0], tracked.tasks[0]);
  assert.equal(destroyed, 0);
  finish();
  await tracked.tasks[0];
  assert.equal(destroyed, 1);
  assert.equal(native.waitUntil, nativeWaitUntil);
  assert.equal(
    trackExecutionContext(native).context.waitUntil === patched,
    false,
  );
  assert.equal(Reflect.deleteProperty(context, "waitUntil"), true);
  assert.equal(context.waitUntil, original);
  Object.defineProperty(context, "waitUntil", {
    value: patched,
    writable: false,
  });
  assert.equal(context.waitUntil, patched);
  assert.equal(Reflect.set(context, "waitUntil", original), false);
  assert.equal(Reflect.deleteProperty(context, "waitUntil"), false);
  tracked.extendLifetime(Promise.resolve());
  assert.equal(registered.length, 2);
});

test("imported waitUntil drains nested and failed work in its own HTTP root", async () => {
  const before = completions.length;
  const events = [Promise.withResolvers(), Promise.withResolvers()];
  const nested = Promise.withResolvers();
  const waits = [[], []];
  const wrapped = wrapDefault({
    async fetch(request) {
      const index = Number(new URL(request.url).pathname.slice(1));
      await Promise.resolve();
      const nativePush = Array.prototype.push;
      Array.prototype.push = () => {
        throw new Error("tenant array hook");
      };
      try {
        cloudflareModule.waitUntil(
          events[index].promise.then(() => {
            if (index === 0) cloudflareModule.waitUntil(nested.promise);
          }),
        );
      } finally {
        Array.prototype.push = nativePush;
      }
      cloudflareModule.waitUntil(
        Promise.reject(new Error("background failure")),
      );
      return new Response("returned");
    },
  });
  for (const index of [0, 1]) {
    const response = await wrapped.fetch(
      new Request(`https://test/${index}`),
      {},
      {
        waitUntil(promise) {
          cloudflareModule.waitUntil(promise);
          waits[index].push(promise);
        },
      },
    );
    assert.equal(await response.text(), "returned");
  }
  assert.equal(completions.length, before);
  events[1].resolve();
  await Promise.all(waits[1]);
  assert.equal(completions.length, before + 1);
  events[0].resolve();
  await events[0].promise;
  assert.equal(completions.length, before + 1);
  nested.resolve();
  await Promise.all(waits[0]);
  assert.equal(completions.length, before + 2);
  assert.equal(cloudflareModule.waitUntilObservers.getStore(), undefined);
});

test("HTTP root retains imported work registered while producing the response stream", async () => {
  const before = completions.length;
  const stream = Promise.withResolvers();
  const background = Promise.withResolvers();
  const waits = [];
  const wrapped = wrapDefault({
    fetch() {
      return new Response(
        new ReadableStream({
          async start(controller) {
            await stream.promise;
            cloudflareModule.waitUntil(background.promise);
            controller.enqueue(new TextEncoder().encode("streamed"));
            controller.close();
          },
        }),
      );
    },
  });
  const response = await wrapped.fetch(
    new Request("https://test/"),
    {},
    {
      waitUntil(promise) {
        cloudflareModule.waitUntil(promise);
        waits.push(promise);
      },
    },
  );
  assert.equal(completions.length, before);
  stream.resolve();
  assert.equal(await response.text(), "streamed");
  await new Promise((resolve) => setImmediate(resolve));
  try {
    assert.equal(completions.length, before);
  } finally {
    background.resolve();
    await Promise.all(waits);
  }
  assert.equal(completions.length, before + 1);
});

test("saved contexts register background work in the current HTTP root", async () => {
  const before = completions.length;
  const background = Promise.withResolvers();
  const waits = [[], []];
  let saved;
  const wrapped = wrapDefault({
    fetch(request, _env, ctx) {
      if (new URL(request.url).pathname === "/capture") saved = ctx;
      else saved.waitUntil(background.promise);
      return new Response("returned");
    },
  });
  for (const [index, path] of ["capture", "use"].entries()) {
    const response = await wrapped.fetch(
      new Request(`https://test/${path}`),
      {},
      {
        waitUntil(promise) {
          cloudflareModule.waitUntil(promise);
          waits[index].push(promise);
        },
      },
    );
    assert.equal(await response.text(), "returned");
    if (index === 0) await Promise.all(waits[0]);
  }
  await new Promise((resolve) => setImmediate(resolve));
  try {
    assert.equal(completions.length, before + 1);
  } finally {
    background.resolve();
    await Promise.all(waits.flat());
  }
  assert.equal(completions.length, before + 2);
});

test("entrypoint constructors retain imported and context background work", async () => {
  const before = completions.length;
  const tasks = [
    Promise.withResolvers(),
    Promise.withResolvers(),
    Promise.withResolvers(),
  ];
  const waits = [];
  const frames = [];
  class Tenant extends cloudflareModule.WorkerEntrypoint {
    constructor(ctx, env) {
      super(ctx, env);
      frames.push(serviceScopeState.currentServiceFrame());
      cloudflareModule.waitUntil(
        tasks[0].promise.then(() => ctx.waitUntil(tasks[2].promise)),
      );
      ctx.waitUntil(tasks[1].promise);
    }
    fetch() {
      frames.push(serviceScopeState.currentServiceFrame());
      return new Response("returned");
    }
  }
  const Wrapped = wrapEntrypoint(Tenant);
  const instance = new Wrapped(
    {
      waitUntil(promise) {
        cloudflareModule.waitUntil(promise);
        waits.push(promise);
      },
    },
    {},
  );
  assert.equal(
    await (await instance.fetch(new Request("https://test/"))).text(),
    "returned",
  );
  assert.equal(completions.length, before);
  tasks[0].resolve();
  await new Promise((resolve) => setImmediate(resolve));
  try {
    assert.equal(completions.length, before);
  } finally {
    tasks[1].resolve();
    tasks[2].resolve();
    await Promise.all(waits);
  }
  assert.equal(completions.length, before + 1);
  assert.equal(frames[0], frames[1]);
  await (await instance.fetch(new Request("https://test/again"))).text();
  await Promise.all(waits);
  assert.notEqual(frames[1].scopeId, frames[2].scopeId);
  assert.throws(
    () => serviceScopeState.currentServiceFrame(),
    /SERVICE_BINDING_DENIED/,
  );
});

test("reused entrypoints keep background tasks in their own dispatch root", async () => {
  const before = completions.length;
  const tasks = [Promise.withResolvers(), Promise.withResolvers()];
  const waits = [];
  class Tenant extends cloudflareModule.WorkerEntrypoint {
    async hold(index) {
      await Promise.resolve();
      this.ctx.waitUntil(tasks[index].promise);
      return index;
    }
  }
  const Wrapped = wrapEntrypoint(Tenant);
  const instance = new Wrapped(
    {
      waitUntil(promise) {
        cloudflareModule.waitUntil(promise);
        waits.push(promise);
      },
    },
    {},
  );
  assert.deepEqual(
    await Promise.all([instance.hold(0), instance.hold(1)]),
    [0, 1],
  );
  tasks[1].resolve();
  await new Promise((resolve) => setImmediate(resolve));
  try {
    assert.equal(completions.length, before + 1);
  } finally {
    tasks[0].resolve();
    await Promise.all(waits);
  }
  assert.equal(completions.length, before + 2);
});

test("tenant ctx.exports and importable exports expose no private generated entrypoints", async () => {
  const native = {
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      promise.catch(() => undefined);
    },
  };
  const context = trackExecutionContext(native).context;
  const exported = context.exports;
  assert.deepEqual(exported.PublicEntrypoint({ props: { value: 42 } }), {
    value: 42,
  });
  assert.deepEqual(exported.SocketService(), { specialized: true });
  const connect = exported.SocketService.connect;
  assert.equal(connect("connected"), "connected");
  assert.equal(exported.SocketService.connect, connect);
  for (const name of ["__OpenComputeDefaultService"]) {
    assert.equal(exported[name], undefined);
    assert.equal(name in exported, false);
    assert.equal(Object.getOwnPropertyDescriptor(exported, name), undefined);
  }
  assert.deepEqual(Reflect.ownKeys(exported), [
    "PublicEntrypoint",
    "SocketService",
  ]);
  assert.deepEqual(Object.keys(exported), [
    "PublicEntrypoint",
    "SocketService",
  ]);
  assert.equal(Object.getPrototypeOf(exported), null);
  assert.equal(Object.getPrototypeOf(context), null);
  assert.equal(
    Object.getOwnPropertyDescriptor(context, "exports").value,
    exported,
  );
  const wrapped = wrapDefault({
    fetch() {
      assert.equal(
        cloudflareModule.exports.__OpenComputeDefaultService,
        undefined,
      );
      assert.deepEqual(Reflect.ownKeys(cloudflareModule.exports), [
        "PublicEntrypoint",
        "SocketService",
      ]);
      return new Response("safe");
    },
  });
  assert.equal(
    await (
      await wrapped.fetch(new Request("https://example.invalid/"), {}, native)
    ).text(),
    "safe",
  );
});

test("tenant prototype edits cannot reveal private generated exports", () => {
  const originalStartsWith = String.prototype.startsWith;
  const originalSetHas = Set.prototype.has;
  const originalWeakHas = WeakSet.prototype.has;
  String.prototype.startsWith = () => false;
  Set.prototype.has = () => true;
  WeakSet.prototype.has = () => false;
  try {
    const exported = trackExecutionContext({}).context.exports;
    assert.equal(exported.__OpenComputeDefaultService, undefined);
    assert.deepEqual(Reflect.ownKeys(exported), [
      "PublicEntrypoint",
      "SocketService",
    ]);
    assert.deepEqual(exported.PublicEntrypoint({ props: { value: 42 } }), {
      value: 42,
    });
  } finally {
    String.prototype.startsWith = originalStartsWith;
    Set.prototype.has = originalSetHas;
    WeakSet.prototype.has = originalWeakHas;
  }
});

test("tenant Reflect edits cannot intercept generated export authority", () => {
  const source = {
    PublicEntrypoint: () => 7,
    __OpenComputeDefaultService: () => 9,
  };
  const originalGet = Reflect.get;
  const originalOwnKeys = Reflect.ownKeys;
  let intercepted = false;
  Reflect.get = (target, ...args) => {
    if (target === source) intercepted = true;
    return originalGet(target, ...args);
  };
  Reflect.ownKeys = (target) => {
    if (target === source) intercepted = true;
    return originalOwnKeys(target);
  };
  try {
    const exported = loopbackModule.tenantExports(source);
    assert.equal(exported.__OpenComputeDefaultService, undefined);
    assert.equal(exported.PublicEntrypoint(), 7);
    assert.equal(intercepted, false);
  } finally {
    Reflect.get = originalGet;
    Reflect.ownKeys = originalOwnKeys;
  }
});

test("object and function handlers restore async env scope and preserve event receivers", async () => {
  const handler = {
    label: "owner",
    async fetch(_request, env) {
      await Promise.resolve();
      assert.equal(scope.getStore().TOKEN, env.TOKEN);
      assert.equal(env.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, undefined);
      return this.label;
    },
    scheduled(event) {
      assert.equal(event.type, "scheduled");
      return event.read();
    },
    trace(event, env, ctx) {
      assert.equal(this, handler);
      assert.equal(env.TRACE, "ok");
      assert.equal(typeof ctx.waitUntil, "function");
      return event.read();
    },
  };
  const wrapped = wrapDefault(handler);
  const context = {
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      promise.catch(() => undefined);
    },
  };
  assert.equal(
    await wrapped.fetch(
      new Request("https://example.invalid"),
      { TOKEN: "value" },
      context,
    ),
    "owner",
  );
  class Event {
    #value = 42;
    read() {
      return this.#value;
    }
  }
  assert.equal(wrapped.scheduled(new Event(), {}, context), 42);
  assert.equal(wrapped.trace(new Event(), { TRACE: "ok" }, context), 42);
  const fn = wrapDefault((_event, env) => env.MESSAGE);
  assert.equal(fn.fetch({}, { MESSAGE: "ok" }, context), "ok");
  assert.equal(scope.getStore(), undefined);
});

test("fetch wrappers preserve the native subrequest-limit outcome across WorkerLoader", async () => {
  const context = { waitUntil() {} };
  for (const handler of [
    {
      async fetch() {
        throw new Error("Too many subrequests.");
      },
    },
    async () => {
      throw new Error("Too many subrequests.");
    },
  ]) {
    const wrapped = wrapDefault(handler);
    const response = await wrapped.fetch(
      new Request("https://example.invalid/"),
      {},
      context,
    );
    assert.equal(response.status, 500);
    assert.equal(
      response.headers.get("x-open-compute-resource-limit"),
      "subrequests",
    );
  }
  class Entrypoint {
    async fetch() {
      throw new Error("Too many subrequests.");
    }
  }
  const Wrapped = wrapDefault(Entrypoint);
  const response = await new Wrapped(context, {}).fetch(
    new Request("https://example.invalid/"),
  );
  assert.equal(
    response.headers.get("x-open-compute-resource-limit"),
    "subrequests",
  );
});

test("direct Workflow schedules run before the optional tenant handler and hide trusted targets", async () => {
  const start = workflowFacadeState.scheduledCalls.length;
  const flow = { binding: "FLOW" };
  const context = {
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      promise.catch(() => undefined);
    },
  };
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
      assert.equal(
        Reflect.ownKeys(controller).includes("workflowBindings"),
        false,
      );
      controller.noRetry();
      return "tenant-result";
    },
  };
  const wrapped = wrapDefault(handler, undefined, {
    targets: [
      {
        cron: "*/5 * * * *",
        scheduledHandler: true,
        workflowBindings: ["FLOW"],
      },
      {
        cron: "0 * * * *",
        scheduledHandler: false,
        workflowBindings: ["FLOW"],
      },
    ],
    trigger: workflowFacadeState.triggerWorkflowSchedule,
  });
  let noRetry = 0;
  const event = {
    scheduledTime: 1_788_048_000_000,
    cron: "*/5 * * * *",
    noRetry() {
      noRetry += 1;
    },
  };
  assert.equal(
    await wrapped.scheduled(event, { FLOW: flow }, context),
    "tenant-result",
  );
  assert.equal(invoked, 1);
  assert.equal(noRetry, 1);
  assert.deepEqual(workflowFacadeState.scheduledCalls.slice(start), [
    {
      binding: flow,
      schedule: { cron: "*/5 * * * *", scheduledTime: 1_788_048_000_000 },
    },
  ]);

  const workflowOnly = {
    ...event,
    cron: "0 * * * *",
  };
  assert.equal(
    await wrapped.scheduled(workflowOnly, { FLOW: flow }, context),
    undefined,
  );
  assert.equal(invoked, 1);
  assert.equal(workflowFacadeState.scheduledCalls.length, start + 2);
  const invalid = wrapDefault(handler, undefined, {
    targets: [
      {
        cron: "*/5 * * * *",
        scheduledHandler: true,
        workflowBindings: ["FLOW", "FLOW"],
      },
    ],
    trigger: workflowFacadeState.triggerWorkflowSchedule,
  });
  await assert.rejects(
    invalid.scheduled(event, { FLOW: flow }, context),
    /CRON_CUSTOM_EVENT_UNSUPPORTED/,
  );
});

test("scheduled handlers receive public env and ctx with and without workflow policy", async () => {
  for (const workflows of [false, true]) {
    for (const proxy of [false, true]) {
      const environment = { PUBLIC_VALUE: "scoped", FLOW: {} };
      let calls = 0;
      function scheduled(event, env, ctx) {
        calls++;
        assert.equal(event.type, "scheduled");
        assert.equal(event.cron, "0 * * * *");
        assert.equal(event.scheduledTime, 1_788_048_000_000);
        assert.equal(env, environment);
        assert.equal(ctx.publicMarker, "context");
        assert.equal(typeof ctx.waitUntil, "function");
        return "scheduled-result";
      }
      class Tenant {
        constructor() {
          if (proxy)
            return new Proxy(this, {
              get(target, key, receiver) {
                return key === "scheduled"
                  ? scheduled
                  : Reflect.get(target, key, receiver);
              },
            });
        }
        scheduled(...args) {
          return scheduled(...args);
        }
      }
      const policy = workflows
        ? {
            targets: [
              {
                cron: "0 * * * *",
                scheduledHandler: true,
                workflowBindings: ["FLOW"],
              },
            ],
            trigger: workflowFacadeState.triggerWorkflowSchedule,
          }
        : undefined;
      const Wrapped = wrapEntrypoint(Tenant, undefined, undefined, policy);
      const instance = new Wrapped(
        { publicMarker: "context", waitUntil: cloudflareModule.waitUntil },
        environment,
      );
      assert.equal(
        await instance.scheduled({
          cron: "0 * * * *",
          scheduledTime: 1_788_048_000_000,
        }),
        "scheduled-result",
      );
      assert.equal(calls, 1);
    }
  }
});

test("scheduled class policies dispatch the original handler on ordinary and constructor Proxy instances", async () => {
  const start = workflowFacadeState.scheduledCalls.length;
  const flow = {};
  for (const proxy of [false, true]) {
    let calls = 0;
    function scheduled(event) {
      calls++;
      assert.equal(event.type, "scheduled");
      assert.equal(event.scheduledHandler, undefined);
      assert.equal(event.workflowBindings, undefined);
      return "scheduled-result";
    }
    class Tenant {
      constructor() {
        if (proxy)
          return new Proxy(this, {
            get(target, key, receiver) {
              if (key === "scheduled") return scheduled;
              return Reflect.get(target, key, receiver);
            },
          });
      }
      scheduled(event) {
        return scheduled(event);
      }
    }
    const Wrapped = wrapEntrypoint(Tenant, undefined, undefined, {
      targets: [
        {
          cron: "0 * * * *",
          scheduledHandler: true,
          workflowBindings: ["FLOW"],
        },
      ],
      trigger: workflowFacadeState.triggerWorkflowSchedule,
    });
    const instance = new Wrapped(
      {
        waitUntil(value) {
          cloudflareModule.waitUntil(value);
          value.catch(() => {});
        },
      },
      { FLOW: flow },
    );
    assert.equal(
      await instance.scheduled({
        cron: "0 * * * *",
        scheduledTime: 1_788_048_000_000,
      }),
      "scheduled-result",
    );
    assert.equal(calls, 1);
  }
  assert.equal(workflowFacadeState.scheduledCalls.length, start + 2);
});

test("class construction, async RPC and private fields keep their native receivers and env", async () => {
  let constructed;
  class Tenant {
    #value;
    constructor(ctx, env) {
      assert.equal(scope.getStore().TOKEN, env.TOKEN);
      constructed = env;
      this.#value = ctx;
    }
    async read() {
      await Promise.resolve();
      assert.equal(scope.getStore().TOKEN, constructed.TOKEN);
      return this.#value;
    }
  }
  const Wrapped = wrapEntrypoint(Tenant, "Named");
  const context = {
    value: 42,
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      promise.catch(() => undefined);
    },
  };
  const instance = new Wrapped(context, {
    TOKEN: "value",
  });
  assert.equal(Wrapped.name, "Named");
  assert.ok(instance instanceof Tenant);
  assert.equal((await instance.read()).value, 42);
  assert.equal(constructed.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX, undefined);
  assert.equal(scope.getStore(), undefined);
  const Default = wrapDefault(Tenant);
  assert.equal(
    (await new Default({ ...context, value: 43 }, {}).read()).value,
    43,
  );
  for (const invalid of [null, {}, () => {}])
    assert.throws(() => wrapEntrypoint(invalid), /missing entrypoint/);
});

for (const operation of ["rpc", "getter", "getter failure"]) {
  test(`Service ${operation} preserves host methods on constructor proxies and returns before background drains`, async () => {
    const before = completions.length;
    const frame = { scopeId: crypto.randomUUID(), frame: crypto.randomUUID() };
    const constructorTask = Promise.withResolvers();
    let finish;
    const background = new Promise((resolve) => {
      finish = resolve;
    });
    class Tenant {
      constructor(ctx) {
        this.ctx = ctx;
        assert.deepEqual(serviceScopeState.currentServiceFrame(), {
          scopeId: frame.scopeId,
          parentFrame: frame.frame,
        });
        assert.deepEqual(ctx.props, { tenant: true });
        ctx.waitUntil(constructorTask.promise);
        return new Proxy(this, {
          get(target, key, receiver) {
            if (typeof key === "string" && key.startsWith("__openCompute"))
              return () => {
                throw new Error("host method reached tenant proxy");
              };
            return Reflect.get(target, key, receiver);
          },
        });
      }
      operation() {
        return this.property;
      }
      get property() {
        assert.deepEqual(serviceScopeState.currentServiceFrame(), {
          scopeId: frame.scopeId,
          parentFrame: frame.frame,
        });
        cloudflareModule.waitUntil(background);
        if (operation === "getter failure") throw new Error("property failed");
        return 42;
      }
    }
    const context = {
      props: {
        __OPEN_COMPUTE_SERVICE_CONTEXT: frame,
        userProps: { tenant: true },
      },
      waitUntil(promise) {
        cloudflareModule.waitUntil(promise);
        promise.catch(() => undefined);
      },
    };
    const instance = new (wrapEntrypoint(Tenant))(context, {});
    const reporter = {
      beginCapability() {},
      releaseRetention() {},
      completeOperation() {},
      retainCapability() {},
      dup() {
        return this;
      },
      [Symbol.dispose]() {},
    };
    const envelope =
      operation === "rpc"
        ? await instance.__openComputeServiceRpc(
            frame.scopeId,
            frame.frame,
            reporter,
            "operation",
            [],
          )
        : await instance.__openComputeServiceGet(
            frame.scopeId,
            frame.frame,
            reporter,
            "property",
          );
    assert.equal(envelope.ok, operation !== "getter failure");
    if (envelope.ok) assert.equal(envelope.value, 42);
    else assert.equal(envelope.error.message, "property failed");
    const reader = envelope.background.getReader();
    let drained = false;
    const read = reader.read().then((part) => {
      drained = part.done;
    });
    await Promise.resolve();
    assert.equal(drained, false);
    finish();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(drained, false);
    assert.equal(completions.length, before);
    constructorTask.resolve();
    await read;
    assert.equal(drained, true);
    assert.equal(completions.length, before);
  });
}

test("Service entrypoint visibility rejects class own fields without evaluating them", async () => {
  let effects = 0;
  class Tenant extends cloudflareModule.WorkerEntrypoint {
    data = 9;
    arrow = () => {
      effects++;
      return 9;
    };
    constructor(ctx, env) {
      super(ctx, env);
      Object.defineProperty(this, "ownAccessor", {
        get() {
          effects++;
          return 9;
        },
      });
    }
    get value() {
      return 3;
    }
    inherited() {
      return 4;
    }
  }
  const context = {
    props: {},
    waitUntil(promise) {
      promise.catch(() => undefined);
    },
  };
  const reporter = {
    retainCapability() {},
    beginCapability() {},
    releaseRetention() {},
    completeOperation() {},
  };
  for (const [operation, member] of [
    ["get", "data"],
    ["call", "arrow"],
    ["get", "ownAccessor"],
    ["get", "value"],
    ["call", "inherited"],
  ]) {
    const instance = new (wrapEntrypoint(Tenant))(context, {});
    const scopeId = crypto.randomUUID(),
      frame = crypto.randomUUID();
    const envelope =
      operation === "call"
        ? await instance.__openComputeServiceRpc(
            scopeId,
            frame,
            reporter,
            member,
            [],
          )
        : await instance.__openComputeServiceGet(
            scopeId,
            frame,
            reporter,
            member,
          );
    const visible = member === "value" || member === "inherited";
    assert.equal(envelope.ok, visible);
    if (visible) assert.equal(envelope.value, member === "value" ? 3 : 4);
    else assert.ok(envelope.error instanceof TypeError);
    assert.equal((await envelope.background.getReader().read()).done, true);
  }
  assert.equal(effects, 0);
});

test("object and function default Service fetches receive the target env and context", async () => {
  const pending = [];
  const context = {
    props: {
      __OPEN_COMPUTE_SERVICE_CONTEXT: {
        scopeId: crypto.randomUUID(),
        frame: crypto.randomUUID(),
        completion: {
          async fetch() {
            return new Response(null, { status: 204 });
          },
        },
      },
    },
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      pending.push(promise);
    },
  };
  const object = {
    fetch(request, env, ctx) {
      ctx.waitUntil(Promise.resolve());
      return new Response(
        `${this === object}:${env.OWNER}:${new URL(request.url).hostname}`,
      );
    },
  };
  for (const [raw, expected] of [
    [object, "true:object:service.example"],
    [
      (_request, env) => new Response(`function:${env.OWNER}`),
      "function:function",
    ],
  ]) {
    const DefaultService = wrapDefaultService(raw);
    const instance = new DefaultService(context, {
      OWNER: expected.startsWith("true") ? "object" : "function",
    });
    const response = await instance.fetch(
      new Request("https://service.example/path"),
    );
    assert.equal(await response.text(), expected);
    await Promise.all(pending);
  }
});

test("Service WebSocket fetch hands the native socket off without completing the operation", async () => {
  let completed = 0;
  const pending = [];
  const socket = new EventTarget();
  const context = {
    props: {
      __OPEN_COMPUTE_SERVICE_CONTEXT: {
        scopeId: crypto.randomUUID(),
        frame: crypto.randomUUID(),
        completion: {
          fetch() {
            completed += 1;
            return new Response(null, { status: 204 });
          },
        },
      },
    },
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      pending.push(promise);
    },
  };
  const DefaultService = wrapDefaultService({
    fetch() {
      const response = new Response(null, { status: 200 });
      Object.defineProperty(response, "webSocket", { value: socket });
      return response;
    },
  });

  const response = await new DefaultService(context, {}).fetch(
    new Request("https://service.invalid/socket"),
  );
  assert.equal(response.webSocket, socket);
  await Promise.all(pending);
  assert.equal(completed, 0);
});

test("object default Service connect receives the native socket, target env, and context", async () => {
  const context = {
    marker: "ctx",
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      promise.catch(() => undefined);
    },
  };
  const socket = { native: true };
  const object = {
    async connect(actual, env, ctx) {
      await Promise.resolve();
      assert.equal(this, object);
      assert.equal(actual, socket);
      assert.equal(env.OWNER, "object");
      assert.equal(ctx.marker, "ctx");
      assert.equal(scope.getStore().TOKEN, env.TOKEN);
    },
  };
  const DefaultService = wrapDefaultService(object);
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
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      promise.catch(() => undefined);
    },
  };
  const cacheEnvironments = [];
  const Entry = createWorkflowEntrypoint(
    target,
    async (actual, ctx, env, event, backend) => {
      assert.equal(actual, target);
      assert.equal(ctx, context);
      assert.equal(ctx.context, true);
      assert.equal(scope.getStore().TOKEN, env.TOKEN);
      assert.equal(backend, controller);
      assert.deepEqual(env, { USER: "public" });
      assert.deepEqual(event, { payloadJson: "null" });
      return { outcome: "complete", outputJson: "42", finalOrdinal: 0 };
    },
    (value) => value === target,
    {
      bind(env) {
        cacheEnvironments.push(env);
        return undefined;
      },
    },
  );
  const entry = new Entry(context, {
    USER: "public",
  });
  assert.equal(entry.ctx, context);
  assert.equal(entry.validate(), true);
  assert.equal(
    (await entry.execute({ payloadJson: "null" }, controller)).outcome,
    "complete",
  );
  for (
    let attempt = 0;
    attempt < 10 && completions.length === priorCompletions;
    attempt += 1
  ) {
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.equal(serviceScopeState.scopeRuns, priorScopes + 1);
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(cacheEnvironments.length, 1);
  assert.equal(cacheEnvironments[0], undefined);
  assert.equal(scope.getStore(), undefined);
});

test("Durable Object methods share the root Service scope and tracked waitUntil lifecycle", async () => {
  const priorScopes = serviceScopeState.scopeRuns;
  const priorCompletions = completions.length;
  const frames = [];
  class Capability {
    constructor(raw) {
      this.value = raw;
    }
  }
  class Tenant {
    rpcValue() {
      return "rpc-value";
    }
    get rpcProperty() {
      return "rpc-property";
    }
    constructor(ctx, env) {
      frames.push(serviceScopeState.currentServiceFrame());
      this.ctx = ctx;
      this.env = env;
      this.privateExportsHidden =
        ctx.exports.__OpenComputeDefaultService === undefined;
      assert.equal(env.__OPEN_COMPUTE_PRIVATE_NATIVE_FACETS, undefined);
      assert.equal(
        Object.keys(env).some((name) =>
          name.startsWith("__OPEN_COMPUTE_PRIVATE_"),
        ),
        false,
      );
      return new Proxy(this, {
        get(target, key, receiver) {
          if (typeof key === "string" && key.startsWith("__openCompute"))
            return () => {
              throw new Error("host alarm reached tenant proxy");
            };
          return Reflect.get(target, key, receiver);
        },
      });
    }
    alarm() {
      return "alarm-fired";
    }
    async fetch() {
      await Promise.resolve();
      frames.push(serviceScopeState.currentServiceFrame());
      this.ctx.waitUntil(Promise.resolve());
      return `${this.env.VALUE}:${this.ctx.storage}:${this.env.OBJECTS.value}:${this.privateExportsHidden}`;
    }
  }
  const cacheEnvironments = [];
  const privateEnvironment = {};
  const Wrapped = wrapDurableObject(Tenant, privateEnvironment, "Object", {
    bind(env) {
      cacheEnvironments.push(env);
      return undefined;
    },
  });
  let contextWaits = 0;
  const context = {
    get storage() {
      if (this !== context)
        throw new TypeError("invalid native context receiver");
      return "native";
    },
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
      contextWaits += 1;
      promise.catch(() => undefined);
    },
    exports: cloudflareModule.workerExports,
  };
  const index = { upsert() {}, delete() {}, clear() {} };
  const facetManager = {
    __openComputeFacetCall() {},
    __openComputeFacetClone() {},
  };
  const facetAuthority = {
    instanceId: "account",
    workerId: "worker",
    versionId: "version",
    workerCodeSha256: "a".repeat(64),
    className: "Object",
  };
  Object.assign(privateEnvironment, {
    __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: index,
    __OPEN_COMPUTE_PRIVATE_FACET_MANAGER: facetManager,
    __OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY: facetAuthority,
    __OPEN_COMPUTE_PRIVATE_FACET_PATH: [],
    __OPEN_COMPUTE_PRIVATE_FACET_PROPS: undefined,
    __OPEN_COMPUTE_PRIVATE_NATIVE_FACETS: { create() {}, revoke() {} },
  });
  const instance = cloudflareModule.withExports(
    { PublicEntrypoint: cloudflareModule.workerExports.PublicEntrypoint },
    () =>
      new Wrapped(context, {
        VALUE: "ok",
        OBJECTS: new Capability("trusted", true),
      }),
  );
  assert.equal(await instance.fetch(), "ok:native:trusted:true");
  assert.equal(context.exports.__OpenComputeDefaultService, undefined);
  assert.equal(
    Object.getOwnPropertyDescriptor(context, "exports").configurable,
    false,
  );
  for (
    let attempt = 0;
    attempt < 10 && completions.length === priorCompletions;
    attempt += 1
  ) {
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.equal(serviceScopeState.scopeRuns, priorScopes + 2);
  assert.equal(frames[0], frames[1]);
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(contextWaits, 2);
  assert.equal(cacheEnvironments.length, 1);
  assert.equal(cacheEnvironments[0], undefined);
  const priorAlarms = alarmShimState.alarmCalls.length;
  assert.equal(
    await instance.__openComputeAlarm({ scheduledTimeMs: 1 }),
    "alarm-fired",
  );
  await instance.__openComputeAlarmRepair();
  assert.equal(alarmShimState.alarmCalls.length, priorAlarms + 2);
  assert.deepEqual(alarmShimState.alarmCalls[priorAlarms].payload, {
    scheduledTimeMs: 1,
  });
  assert.ok(alarmShimState.alarmCalls[priorAlarms + 1].repair);
  let admitted = 0;
  assert.equal(
    await instance.__openComputeInvokeRpc("call", "rpcValue", [], async () => {
      admitted += 1;
    }),
    "rpc-value",
  );
  assert.equal(
    await instance.__openComputeInvokeRpc(
      "get",
      "rpcProperty",
      [],
      async () => {
        admitted += 1;
      },
    ),
    "rpc-property",
  );
  assert.equal(admitted, 2);
  await assert.rejects(
    instance.__openComputeInvokeRpc(
      "call",
      "__openComputeAlarm",
      [],
      async () => {},
    ),
    /DO_RPC_UNSUPPORTED/,
  );
});

test("Durable Object WebSocket responses hand ownership to native hibernation", async () => {
  const priorCompletions = completions.length;
  class Tenant {
    fetch() {
      const response = new Response(null, { status: 200 });
      Object.defineProperty(response, "webSocket", {
        value: new EventTarget(),
      });
      return response;
    }
  }
  const privateEnvironment = {};
  const Wrapped = wrapDurableObject(Tenant, privateEnvironment, "SocketObject");
  const waits = [];
  Object.assign(privateEnvironment, {
    __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: {
      upsert() {},
      delete() {},
      clear() {},
    },
    __OPEN_COMPUTE_PRIVATE_FACET_MANAGER: {
      __openComputeFacetCall() {},
      __openComputeFacetClone() {},
    },
    __OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY: {
      instanceId: "account",
      workerId: "worker",
      versionId: "version",
      workerCodeSha256: "a".repeat(64),
      className: "SocketObject",
    },
    __OPEN_COMPUTE_PRIVATE_FACET_PATH: [],
    __OPEN_COMPUTE_PRIVATE_FACET_PROPS: undefined,
    __OPEN_COMPUTE_PRIVATE_NATIVE_FACETS: { create() {}, revoke() {} },
  });
  const instance = new Wrapped(
    {
      storage: "native",
      exports: cloudflareModule.workerExports,
      waitUntil(promise) {
        cloudflareModule.waitUntil(promise);
        waits.push(promise);
        promise.catch(() => undefined);
      },
    },
    {},
  );
  const response = instance.fetch();
  assert.ok(response.webSocket instanceof EventTarget);
  for (
    let attempt = 0;
    attempt < 10 && completions.length === priorCompletions;
    attempt += 1
  ) {
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.equal(completions.length, priorCompletions + 1);
  assert.equal(waits.length, 1);
});

test("native Service fetch hides transport props and completes after both response and background drain", async () => {
  for (const named of [true, false]) {
    let finish;
    let completed = 0;
    const pending = [];
    const background = new Promise((resolve) => {
      finish = resolve;
    });
    const stream = Promise.withResolvers();
    const context = {
      props: {
        userProps: { theme: "dark" },
        __OPEN_COMPUTE_SERVICE_CONTEXT: {
          scopeId: crypto.randomUUID(),
          frame: crypto.randomUUID(),
          completion: {
            fetch() {
              completed++;
              return new Response(null, { status: 204 });
            },
          },
        },
      },
      waitUntil(promise) {
        cloudflareModule.waitUntil(promise);
        pending.push(promise);
      },
    };
    const handler = (_request, _env, ctx) => {
      assert.deepEqual(ctx.props, { theme: "dark" });
      return new Response(
        new ReadableStream({
          async start(controller) {
            await stream.promise;
            cloudflareModule.waitUntil(background);
            controller.enqueue(new TextEncoder().encode("streamed"));
            controller.close();
          },
        }),
      );
    };
    class Named {
      constructor(ctx, env) {
        this.ctx = ctx;
        this.env = env;
      }
      fetch(request) {
        return handler(request, this.env, this.ctx);
      }
    }
    const Entry = named
      ? wrapEntrypoint(Named)
      : wrapDefaultService({ fetch: handler });
    const response = await new Entry(context, {}).fetch(
      new Request("https://service.example/"),
    );
    assert.equal(completed, 0);
    stream.resolve();
    assert.equal(await response.text(), "streamed");
    await new Promise((resolve) => setImmediate(resolve));
    try {
      assert.equal(completed, 0);
    } finally {
      finish();
      await Promise.all(pending);
    }
    assert.equal(completed, 1);
  }
});

test("loopback services wrap bindings, hide host capabilities and preserve scoped props", async () => {
  const pending = [];
  class Capability {
    constructor(raw) {
      this.value = raw.value;
    }
    read() {
      return this.value;
    }
  }

  class LoopbackTail extends cloudflareModule.WorkerEntrypoint {
    async tail() {
      return [
        this.env.BINDING.read(),
        this.ctx.props.marker,
        this.env.__OPEN_COMPUTE_PRIVATE_CACHE,
        this.ctx.exports.__OpenComputeLoopbackService,
      ];
    }
  }
  const Bridge = loopbackModule.createLoopbackEntrypoint(
    { LoopbackTail },
    wrapEntrypoint,
  );
  const environment = {
    BINDING: new Capability({ value: "wrapped" }),
  };
  const exports = {
    LoopbackTail() {
      throw new Error("raw loopback entrypoint reached");
    },
    __OpenComputeLoopbackService({ props }) {
      const instance = new Bridge(
        {
          props,
          exports,
          waitUntil(promise) {
            cloudflareModule.waitUntil(promise);
            pending.push(promise);
          },
        },
        environment,
      );
      return { tail: (...args) => instance.tail(...args) };
    },
  };
  const exposed = loopbackModule.tenantExports(exports);
  assert.equal(exposed.__OpenComputeLoopbackService, undefined);
  for (const invalid of [
    1,
    "options",
    { props: null },
    { props: 2 },
    { props: "text" },
  ]) {
    assert.throws(() => exposed.LoopbackTail(invalid), TypeError);
  }
  for (const options of [undefined, null, {}, { props: [] }]) {
    assert.deepEqual(
      await cloudflareModule.withExports(exposed, () =>
        exposed.LoopbackTail(options).tail(),
      ),
      ["wrapped", undefined, undefined, undefined],
    );
  }
  assert.deepEqual(
    await cloudflareModule.withExports(exposed, () =>
      exposed.LoopbackTail({ props: { marker: "scoped" } }).tail(),
    ),
    ["wrapped", "scoped", undefined, undefined],
  );
  assert.deepEqual(
    await cloudflareModule.withExports(exposed, () =>
      exposed
        .LoopbackTail(
          Object.freeze({ props: Object.freeze({ marker: "frozen" }) }),
        )
        .tail(),
    ),
    ["wrapped", "frozen", undefined, undefined],
  );
  let reads = 0;
  const options = {
    get props() {
      reads += 1;
      return { marker: "getter" };
    },
  };
  assert.deepEqual(
    await cloudflareModule.withExports(exposed, () =>
      exposed.LoopbackTail(options).tail(),
    ),
    ["wrapped", "getter", undefined, undefined],
  );
  assert.equal(reads, 1);
  assert.deepEqual(
    await cloudflareModule.withExports(exposed, () =>
      exposed.LoopbackTail.tail(),
    ),
    ["wrapped", undefined, undefined, undefined],
  );
  await Promise.all(pending);
});

test("tenant global edits cannot observe private Service request context", () => {
  const completion = {
    fetch() {
      return new Response(null);
    },
  };
  const native = {
    scopeId: crypto.randomUUID(),
    frame: crypto.randomUUID(),
    completion,
  };
  const props = {
    userProps: { tenant: true },
    __OPEN_COMPUTE_SERVICE_CONTEXT: native,
  };
  const context = { props, exports: {}, waitUntil() {} };
  const secrets = new Set([context, props, native, completion]);
  const leaks = [];
  const get = Reflect.get;
  const apply = Reflect.apply;
  const bind = Function.prototype.bind;
  const ProxyConstructor = globalThis.Proxy;
  const reflection = new Map(
    [
      "has",
      "ownKeys",
      "getOwnPropertyDescriptor",
      "set",
      "defineProperty",
      "deleteProperty",
    ].map((name) => [name, Reflect[name]]),
  );
  let wrappedContext;
  class Tenant extends cloudflareModule.WorkerEntrypoint {
    constructor(ctx, env) {
      super(ctx, env);
      wrappedContext = ctx;
    }
    fetch() {
      return new Response("tenant");
    }
  }
  const Wrapped = wrapEntrypoint(Tenant);
  try {
    Reflect.get = (target, ...args) => {
      if (secrets.has(target)) leaks.push("get");
      return get(target, ...args);
    };
    Reflect.apply = (method, receiver, args) => {
      if (secrets.has(receiver) || args.some((value) => secrets.has(value)))
        leaks.push("apply");
      return apply(method, receiver, args);
    };
    Function.prototype.bind = function (receiver, ...args) {
      if (secrets.has(receiver)) leaks.push("bind");
      return apply(bind, this, [receiver, ...args]);
    };
    globalThis.Proxy = function (target, handler) {
      if (secrets.has(target)) leaks.push("Proxy");
      return new ProxyConstructor(target, handler);
    };
    for (const [name, method] of reflection) {
      Reflect[name] = (target, ...args) => {
        if (secrets.has(target)) leaks.push(name);
        return apply(method, Reflect, [target, ...args]);
      };
    }
    const instance = new Wrapped(context, {});
    assert.equal(instance.env === context, false);
    assert.deepEqual(wrappedContext.props, { tenant: true });
    wrappedContext.waitUntil(Promise.resolve());
    assert.equal(Reflect.has(wrappedContext, "props"), true);
    assert.ok(Object.keys(wrappedContext).includes("exports"));
    assert.deepEqual(
      Object.getOwnPropertyDescriptor(wrappedContext, "props").value,
      { tenant: true },
    );
    assert.equal(Reflect.set(wrappedContext, "tenantValue", 1), true);
    assert.equal(
      Reflect.defineProperty(wrappedContext, "tenantProperty", {
        value: 2,
        configurable: true,
      }),
      true,
    );
    assert.equal(
      Reflect.deleteProperty(wrappedContext, "tenantProperty"),
      true,
    );
  } finally {
    Reflect.get = get;
    Reflect.apply = apply;
    Function.prototype.bind = bind;
    globalThis.Proxy = ProxyConstructor;
    for (const [name, method] of reflection) Reflect[name] = method;
  }
  assert.deepEqual(leaks, []);
});

test("failed constructors finish their root after retained background work", async () => {
  const before = completions.length;
  const task = Promise.withResolvers();
  const pending = [];
  let frame;
  class Tenant {
    constructor(ctx) {
      frame = serviceScopeState.currentServiceFrame();
      ctx.waitUntil(task.promise);
      throw new Error("constructor failed");
    }
  }
  const Wrapped = wrapEntrypoint(Tenant);
  assert.throws(
    () =>
      new Wrapped(
        {
          waitUntil(promise) {
            cloudflareModule.waitUntil(promise);
            pending.push(promise);
          },
        },
        {},
      ),
    /constructor failed/,
  );
  assert.equal(completions.length, before);
  task.resolve();
  await Promise.all(pending);
  assert.deepEqual(completions.slice(before), [frame.scopeId]);
});

test("Service constructor context rejects malformed completion capabilities", () => {
  let constructed = false;
  class Tenant {
    constructor() {
      constructed = true;
    }
  }
  const Wrapped = wrapEntrypoint(Tenant);
  for (const completion of [null, {}, { fetch: 42 }]) {
    assert.throws(
      () =>
        new Wrapped(
          {
            props: {
              __OPEN_COMPUTE_SERVICE_CONTEXT: {
                scopeId: crypto.randomUUID(),
                frame: crypto.randomUUID(),
                completion,
              },
            },
          },
          {},
        ),
      /SERVICE_BINDING_DENIED/,
    );
  }
  assert.equal(constructed, false);
});

test("DO constructor input-gate failure completes its scope before native actor reset", async () => {
  const before = completions.length;
  const failed = Promise.withResolvers();
  let frame;
  let initialized;
  let nativeResult;
  class Tenant {
    constructor(ctx) {
      frame = serviceScopeState.currentServiceFrame();
      assert.throws(
        () => ctx.blockConcurrencyWhile(null),
        /native callback required/,
      );
      initialized = ctx.blockConcurrencyWhile(async () => 42);
      assert.equal(initialized, nativeResult);
      ctx.blockConcurrencyWhile(async () => {
        await Promise.resolve();
        throw new Error("initialization failed");
      });
    }
  }
  const context = {
    blockConcurrencyWhile(callback) {
      assert.equal(this, context);
      if (typeof callback !== "function")
        throw new TypeError("native callback required");
      const nativePromise = Promise.withResolvers();
      nativeResult = nativePromise.promise;
      const execute = async () => {
        try {
          await Promise.resolve();
          nativePromise.resolve(await callback());
        } catch (error) {
          assert.deepEqual(completions.slice(before), [frame.scopeId]);
          failed.resolve(error);
        }
      };
      void execute();
      // Native input-gate failure terminates the actor without rejecting this promise.
      return nativePromise.promise;
    },
    waitUntil(promise) {
      cloudflareModule.waitUntil(promise);
    },
  };
  const privateEnvironment = {
    __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: {
      upsert() {},
      delete() {},
      clear() {},
    },
    __OPEN_COMPUTE_PRIVATE_FACET_MANAGER: {
      __openComputeFacetCall() {},
      __openComputeFacetClone() {},
    },
    __OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY: {
      instanceId: "account",
      workerId: "worker",
      versionId: "version",
      workerCodeSha256: "a".repeat(64),
      className: "Object",
    },
    __OPEN_COMPUTE_PRIVATE_NATIVE_FACETS: { create() {}, revoke() {} },
  };
  new (wrapDurableObject(Tenant, privateEnvironment, "Object"))(context, {});
  assert.equal((await failed.promise).message, "initialization failed");
  assert.equal(await initialized, 42);
  assert.equal(completions.length, before + 1);
});

test("Service entrypoints preserve arbitrary native method and getter property names", async () => {
  const names = ["中文", "with-hyphen", "", "x".repeat(129), "prototype"];
  class Tenant {
    #value = 20;
    value(number) {
      return this.#value + number;
    }
  }
  class Getter {
    #value = 20;
    get value() {
      return this.#value;
    }
  }
  for (const name of names) {
    Object.defineProperty(Tenant.prototype, name, {
      value: Tenant.prototype.value,
    });
    Object.defineProperty(
      Getter.prototype,
      name,
      Object.getOwnPropertyDescriptor(Getter.prototype, "value"),
    );
  }
  const frame = { scopeId: crypto.randomUUID(), frame: crypto.randomUUID() };
  const context = {
    props: { __OPEN_COMPUTE_SERVICE_CONTEXT: frame },
    waitUntil: (promise) => cloudflareModule.waitUntil(promise),
  };
  const reporter = {
    beginCapability() {},
    releaseRetention() {},
    completeOperation() {},
    retainCapability() {},
    dup() {
      return this;
    },
    [Symbol.dispose]() {},
  };
  const instance = new (wrapEntrypoint(Tenant))(context, {});
  const getter = new (wrapEntrypoint(Getter))(context, {});
  for (const name of names) {
    const result = await instance.__openComputeServiceRpc(
      frame.scopeId,
      frame.frame,
      reporter,
      name,
      [2],
    );
    assert.equal(result.ok, true, name);
    assert.equal(result.value, 22);
    assert.equal((await result.background.getReader().read()).done, true);
    const property = await getter.__openComputeServiceGet(
      frame.scopeId,
      frame.frame,
      reporter,
      name,
    );
    assert.equal(property.ok, true, name);
    assert.equal(property.value, 20);
    assert.equal((await property.background.getReader().read()).done, true);
  }
});

test("class admission rejects missing/nonconstructible exports without tenant construction", async () => {
  let calls = 0;
  class Named {
    constructor() {
      calls++;
      throw Error("tenant constructor must not run");
    }
  }
  for (const name of ["Named", "default", "constructor"]) {
    const handler = validationHandler({ [name]: Named }, name, true);
    assert.equal(await handler.fetch().text(), "open-compute-validation-v1");
    assert.equal(calls, 0);
    for (const invalid of [undefined, null, {}, () => {}])
      assert.throws(
        () => validationHandler({ [name]: invalid }, name, true),
        /missing entrypoint/,
      );
    assert.throws(
      () => validationHandler({}, name, true),
      /missing entrypoint/,
    );
    assert.throws(
      () => validationHandler(Object.create({ [name]: Named }), name, true),
      /missing entrypoint/,
    );
  }
  assert.equal(
    await validationHandler({ default: {} }, "default", false).fetch().text(),
    "open-compute-validation-v1",
  );
});

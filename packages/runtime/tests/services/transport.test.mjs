import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  export class RpcTarget {}
  export class WorkerEntrypoint {
    constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  }
  export const background = [];
  export function waitUntil(task) { background.push(task); }
`);
const inert = moduleUrl(`
  export const routeDefaultHttp = () => "worker";
  export const tenantEnv = () => ({});
  export const modulesFor = (snapshot, validation, entrypointName) => ({
    mainModule: snapshot.mainModule, modules: snapshot.modules,
    policy: { validation, entrypointName },
  });
  export const inboundSocketTargetAddress = async () => "example.invalid:443";
  export const tunnelControl = { run: async () => {} };
  export const tunnelSockets = (...args) => tunnelControl.run(...args);
  export const assembleOnce = async (_key, factory) => factory();
  export const bindingError = code => Object.assign(new Error(code), { stableCode: code });
  export const BINDING_TOKEN_HEADER = "x-binding-token";
  export const currentStartupGeneration = () => "generation";
  export const observedEntrypoint = (stub, _factory, _ctx, _identity, name, options) => stub.getEntrypoint(name, options);
  export const doPolicy = () => ({});
  export const INTERNAL_HEADERS = [];
  export const snapshotWorkerCode = () => ({});
  export const snapshotControl = { run: async () => ({ routeGeneration: 1, contentKind: "worker" }) };
  export const resolveSnapshot = (...args) => snapshotControl.run(...args);
  export const tenantGlobalOutbound = () => ({});
  export const tenantLoader = (env) => env.LOADER;
  export async function loadTenantStub(env, _loaderKey, name, code) {
    return { stub: env.LOADER.get(name, code), release() {} };
  }
  export const appendServiceWebSocketHandoff = (response, handle) => {
    response.handoff = handle;
    return response;
  };
`);

globalThis.scheduler = {
  wait: (_delay, options) =>
    options?.signal
      ? new Promise((_resolve, reject) =>
          options.signal.addEventListener(
            "abort",
            () => reject(Error("cancelled deadline")),
            { once: true },
          ),
        )
      : Promise.resolve(),
};

const deadlineUrl = moduleUrl(
  await compileRuntime("services/deadline.ts", {
    "../loader/shared.js": inert,
  }),
);
const transferUrl = moduleUrl(
  await compileRuntime("services/capability-transfer.ts", {
    "cloudflare:workers": cloudflare,
    "../loader/shared.js": inert,
    "./deadline.js": deadlineUrl,
  }),
);
const controlUrl = moduleUrl(
  await compileRuntime("services/control.ts", {
    "cloudflare:workers": cloudflare,
    "../loader/shared.js": inert,
    "./capability-transfer.js": transferUrl,
  }),
);
const transportUrl = moduleUrl(
  await compileRuntime("services/transport.ts", {
    "../loader/policy.js": moduleUrl(await compileRuntime("loader/policy.ts")),
    "cloudflare:workers": cloudflare,
    "../assets/router.js": inert,
    "../loader/bindings.js": inert,
    "../loader/modules.js": inert,
    "../observability/collector.js": inert,
    "./facade.js": inert,
    "./deadline.js": deadlineUrl,
    "./control.js": controlUrl,
    "../sockets/tunnel.js": inert,
    "../loader/shared.js": inert,
  }),
);
const { ServiceTransport } = await import(transportUrl);
const { retryServiceControl } = await import(controlUrl);

function delayedAdmission({ rejectFirst = false } = {}) {
  const firstAdmission = Promise.withResolvers();
  const requests = [];
  const mutations = [];
  let dispatches = 0;
  const ctx = {
    props: {
      versionId: "version",
      bindingName: "FIRST",
      descriptorSha256: "a".repeat(64),
    },
    exports: { ServiceFetchCompletion: () => ({}) },
    waitUntil() {},
  };
  const env = {
    SYSTEM_COMPATIBILITY_DATE: "2026-09-08",
    SYSTEM_COMPATIBILITY_FLAGS: ["experimental"],
    LOADER: {
      get: () => ({
        getEntrypoint: () => ({
          __openComputeServiceRpc: () => ({
            ok: true,
            value: "ok",
            background: new ReadableStream({
              start(controller) {
                controller.close();
              },
            }),
          }),
          fetch: async () => {
            dispatches++;
            return new Response("ok");
          },
        }),
      }),
    },
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: {
      async fetch(url, init) {
        const body = JSON.parse(init.body);
        if (!url.endsWith("/resolve")) {
          mutations.push([url, body]);
          if (url.endsWith("/capabilities/begin"))
            return Response.json({
              handle: crypto.randomUUID(),
              frame: crypto.randomUUID(),
              deadlineMs: 30_000,
            });
          return Response.json({ ok: true });
        }
        requests.push(body);
        if (requests.length === 1) {
          await firstAdmission.promise;
          if (rejectFirst) throw new Error("backend rejected admission");
        }
        return Response.json({
          handle: crypto.randomUUID(),
          frame: crypto.randomUUID(),
          callerFrame: body.parentFrame ?? crypto.randomUUID(),
          deadlineMs: 30_000,
          target: {
            kind: "worker",
            loaderKey: "account/worker/version",
            workerCodeSha256: "a".repeat(64),
            routeGeneration: 1,
            contentKind: "worker",
          },
        });
      },
    },
  };
  return {
    ctx,
    env,
    firstAdmission,
    requests,
    mutations,
    dispatches: () => dispatches,
  };
}

test("concurrent first calls through sibling bindings create one shared root", async (t) => {
  t.mock.method(scheduler, "wait", () => new Promise(() => {}));
  const fixture = delayedAdmission();
  const first = new ServiceTransport(fixture.ctx, fixture.env);
  const sibling = new ServiceTransport(
    { ...fixture.ctx, props: { ...fixture.ctx.props, bindingName: "SECOND" } },
    fixture.env,
  );
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  const calls = [first.rpc(frame, "read", []), sibling.rpc(frame, "read", [])];
  try {
    assert.equal(
      fixture.requests.length,
      1,
      "sibling waits for the root anchor",
    );
    fixture.firstAdmission.resolve();
    assert.deepEqual(await Promise.all(calls), ["ok", "ok"]);
    assert.equal(fixture.requests[0].parentFrame, null);
    assert.notEqual(fixture.requests[1].parentFrame, null);
  } finally {
    fixture.firstAdmission.resolve();
    await Promise.allSettled(calls);
    await first.completeRoot(frame.scopeId);
  }
});

test("abort during HTTP admission releases the late operation without tenant dispatch", async (t) => {
  t.mock.method(scheduler, "wait", () => new Promise(() => {}));
  const fixture = delayedAdmission();
  const transport = new ServiceTransport(fixture.ctx, fixture.env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  const call = transport.fetch(
    new Request("http://service/", {
      headers: { "x-open-compute-service-frame": JSON.stringify(frame) },
    }),
  );
  const closing = Promise.all([
    transport.completeRoot(frame.scopeId),
    transport.completeRoot(frame.scopeId),
  ]);
  fixture.firstAdmission.resolve();
  try {
    await assert.rejects(call, /SERVICE_BINDING_DENIED/);
    await closing;
    assert.equal(fixture.dispatches(), 0);
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
      2,
      "the admitted operation and its root are both released",
    );
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/root/complete"))
        .length,
      1,
      "concurrent completion shares the finalization request",
    );
  } finally {
    await Promise.allSettled([call, closing]);
    await transport.completeRoot(frame.scopeId);
  }
});

test("failed first admission fences siblings until root completion", async (t) => {
  t.mock.method(scheduler, "wait", () => new Promise(() => {}));
  const fixture = delayedAdmission({ rejectFirst: true });
  const transport = new ServiceTransport(fixture.ctx, fixture.env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  const calls = [
    transport.rpc(frame, "read", []),
    transport.rpc(frame, "read", []),
  ];
  fixture.firstAdmission.resolve();
  const outcomes = await Promise.allSettled(calls);
  assert.deepEqual(
    outcomes.map((outcome) => outcome.status),
    ["rejected", "rejected"],
  );
  await assert.rejects(
    transport.rpc(frame, "read", []),
    /SERVICE_BINDING_DENIED/,
  );
  assert.equal(fixture.requests.length, 1);
  await transport.completeRoot(frame.scopeId);
  assert.deepEqual(
    fixture.mutations,
    [],
    "no authority was admitted to release",
  );
  const fresh = { scopeId: crypto.randomUUID(), parentFrame: null };
  assert.equal(await transport.rpc(fresh, "read", []), "ok");
  assert.equal(fixture.requests.length, 2);
  await transport.completeRoot(fresh.scopeId);
});

test("failed root finalization retains its closed identity for an idempotent retry", async (t) => {
  t.mock.method(scheduler, "wait", (delay) =>
    delay < 100 ? Promise.resolve() : new Promise(() => {}),
  );
  const fixture = delayedAdmission();
  const fetch = fixture.env.BINDING_BACKEND.fetch.bind(
    fixture.env.BINDING_BACKEND,
  );
  let failCompletion = true;
  fixture.env.BINDING_BACKEND.fetch = async (url, init) => {
    const response = await fetch(url, init);
    if (url.endsWith("/root/complete") && failCompletion)
      throw new Error("lost finalization response");
    return response;
  };
  const transport = new ServiceTransport(fixture.ctx, fixture.env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  fixture.firstAdmission.resolve();
  assert.equal(await transport.rpc(frame, "read", []), "ok");
  await assert.rejects(
    transport.completeRoot(frame.scopeId),
    /lost finalization response/,
  );
  await assert.rejects(
    transport.rpc(frame, "read", []),
    /SERVICE_BINDING_DENIED/,
  );
  assert.equal(fixture.requests.length, 1);
  failCompletion = false;
  await transport.completeRoot(frame.scopeId);
  const frames = fixture.mutations
    .filter(([url]) => url.endsWith("/root/complete"))
    .map(([, body]) => body.frame);
  assert.equal(frames.length, 4);
  assert.equal(
    new Set(frames).size,
    1,
    "all retries address the original root",
  );
  await transport.completeRoot(frame.scopeId);
  assert.equal(
    fixture.mutations.filter(([url]) => url.endsWith("/root/complete")).length,
    4,
  );
});

test("capability admission waits for the pending root and preserves explicit child frames", async (t) => {
  t.mock.method(scheduler, "wait", () => new Promise(() => {}));
  const fixture = delayedAdmission();
  const transport = new ServiceTransport(fixture.ctx, fixture.env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  const call = transport.rpc(frame, "read", []);
  const capability = transport.beginCapability(crypto.randomUUID(), frame);
  assert.equal(fixture.requests.length, 1);
  assert.deepEqual(fixture.mutations, []);
  fixture.firstAdmission.resolve();
  assert.equal(await call, "ok");
  assert.equal((await capability).deadlineMs, 30_000);
  const child = { ...frame, parentFrame: crypto.randomUUID() };
  await transport.beginCapability(crypto.randomUUID(), child);
  await transport.completeRoot(frame.scopeId);
  const admissions = fixture.mutations.filter(([url]) =>
    url.endsWith("/capabilities/begin"),
  );
  const finalization = fixture.mutations.find(([url]) =>
    url.endsWith("/root/complete"),
  );
  assert.equal(admissions[0][1].parentFrame, finalization[1].frame);
  assert.equal(admissions[1][1].parentFrame, child.parentFrame);
});

test("host root leases reject forged scopes and release late admitted work after disposal", async () => {
  const events = [];
  const lifetime = [];
  const pending = Promise.withResolvers();
  class ControlledTransport extends ServiceTransport {
    rpc(frame) {
      events.push(["rpc", frame.scopeId]);
      return pending.promise;
    }
    async get(frame) {
      events.push(["get", frame.scopeId]);
      if (frame.scopeId === rejectedScope)
        throw new Error("target rejected the operation");
      return "value";
    }
    async completeRoot(scopeId) {
      events.push(["complete", scopeId]);
    }
  }
  const transport = new ControlledTransport(
    {
      props: {
        versionId: "version",
        bindingName: "TARGET",
        descriptorSha256: "a".repeat(64),
      },
      waitUntil(task) {
        lifetime.push(task);
      },
    },
    {},
  );
  assert.throws(() => transport.root("invalid"), /SERVICE_BINDING_DENIED/);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  const rejectedScope = crypto.randomUUID();
  const lease = transport.root(frame.scopeId);
  for (const invalid of [
    { ...frame, scopeId: crypto.randomUUID() },
    { ...frame, parentFrame: crypto.randomUUID() },
    { scopeId: frame.scopeId },
  ]) {
    assert.throws(
      () => lease.rpc(invalid, "read", []),
      /SERVICE_BINDING_DENIED/,
    );
  }
  lease.ready();
  assert.equal(await lease.get(frame, "value"), "value");
  const call = lease.rpc(frame, "read", []);
  lease[Symbol.dispose]();
  lease[Symbol.dispose]();
  assert.throws(() => lease.get(frame, "value"), /SERVICE_BINDING_DENIED/);
  assert.throws(() => lease.ready(), /SERVICE_BINDING_DENIED/);
  const { background } = await import(cloudflare);
  await background.at(-1);
  let lifetimeFinished = false;
  lifetime[0].then(() => {
    lifetimeFinished = true;
  });
  await Promise.resolve();
  assert.equal(lifetimeFinished, false, "late work keeps host cleanup alive");
  pending.resolve("finished");
  assert.equal(await call, "finished");
  await Promise.all(lifetime);
  await Promise.all(background);
  assert.deepEqual(
    events.filter(([event]) => event === "complete"),
    [
      ["complete", frame.scopeId],
      ["complete", frame.scopeId],
    ],
    "early disposal and late admission both reach idempotent root cleanup",
  );
  const rejected = transport.root(rejectedScope);
  await assert.rejects(
    rejected.get({ scopeId: rejectedScope, parentFrame: null }, "value"),
    /target rejected the operation/,
  );
  rejected[Symbol.dispose]();
  await Promise.all(lifetime);
  await Promise.all(background);
});

test("expired Service roots reject repeated and sibling calls without renewing admission", async (t) => {
  let now = 1_000;
  t.mock.method(Date, "now", () => now);
  t.mock.method(scheduler, "wait", () => new Promise(() => {}));
  const resolutions = [];
  const callerFrames = [];
  const completions = [];
  const ctx = {
    props: {
      versionId: "version",
      bindingName: "TARGET",
      descriptorSha256: "a".repeat(64),
    },
  };
  const env = {
    SYSTEM_COMPATIBILITY_DATE: "2026-09-08",
    SYSTEM_COMPATIBILITY_FLAGS: ["experimental"],
    LOADER: {
      get: () => ({
        getEntrypoint: () => ({
          __openComputeServiceRpc: () => ({
            ok: true,
            value: "ok",
            background: new ReadableStream({
              start(controller) {
                controller.close();
              },
            }),
          }),
        }),
      }),
    },
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: {
      async fetch(url, init) {
        const body = JSON.parse(init.body);
        if (url.endsWith("/resolve")) {
          resolutions.push(body);
          callerFrames.push(crypto.randomUUID());
          return Response.json({
            handle: crypto.randomUUID(),
            frame: crypto.randomUUID(),
            callerFrame: callerFrames.at(-1),
            deadlineMs: 30_000,
            target: {
              kind: "worker",
              loaderKey: "account/worker/version",
              workerCodeSha256: "a".repeat(64),
              routeGeneration: 1,
              contentKind: "worker",
            },
          });
        }
        completions.push([url, body]);
        return Response.json({ ok: true });
      },
    },
  };
  const first = new ServiceTransport(ctx, env);
  const sibling = new ServiceTransport(ctx, env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  assert.equal(await first.rpc(frame, "read", []), "ok");
  now += 29_999;
  assert.equal(await sibling.rpc(frame, "read", []), "ok");
  assert.equal(resolutions[1].parentFrame, callerFrames[0]);
  now += 1;
  for (const transport of [first, first, sibling]) {
    await assert.rejects(transport.rpc(frame, "read", []), /SERVICE_TIMEOUT/);
  }
  assert.equal(
    resolutions.length,
    2,
    "expired roots cannot acquire a new budget",
  );
  const fresh = { scopeId: crypto.randomUUID(), parentFrame: null };
  assert.equal(await sibling.rpc(fresh, "read", []), "ok");
  assert.equal(resolutions.length, 3);
  assert.deepEqual(
    resolutions.map((body) => body.parentFrame),
    [null, callerFrames[0], null],
  );
  await first.completeRoot(frame.scopeId);
  await sibling.completeRoot(fresh.scopeId);
  assert.equal(
    completions.filter(([url]) => url.endsWith("/root/complete")).length,
    2,
    "expired identity remains available for the final root cleanup",
  );
  const { background } = await import(cloudflare);
  await Promise.all(background);
});

test("Service lifecycle mutations retry transient private-hop failures", async () => {
  let calls = 0;
  const env = {
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: {
      async fetch(_url, init) {
        calls += 1;
        assert.equal(init.method, "POST");
        if (calls < 3) throw new Error("transient private-hop failure");
        return Response.json({ ok: true });
      },
    },
  };

  assert.deepEqual(
    await retryServiceControl(env, "/internal/services/v1/complete", {
      handle: "operation",
    }),
    { ok: true },
  );
  assert.equal(calls, 3);
});

const { tunnelControl } = await import(inert);

for (const outcome of ["success", "disconnect", "finalization failure"]) {
  test(`Service CONNECT keeps ${outcome} cleanup alive after caller disconnect`, async () => {
    const finalizing = Promise.withResolvers();
    const finish = Promise.withResolvers();
    const background = [];
    let finalizeCalls = 0;
    let closes = 0;
    tunnelControl.run = async () => {
      if (outcome === "disconnect") throw new Error("socket disconnected");
    };
    const ctx = {
      props: {
        versionId: "version",
        bindingName: "TARGET",
        descriptorSha256: "a".repeat(64),
      },
      waitUntil(task) {
        background.push(task);
      },
    };
    const env = {
      LOADER: {
        get() {
          return {
            getEntrypoint() {
              return {
                connect() {
                  return { opened: Promise.resolve(), close: async () => {} };
                },
              };
            },
          };
        },
      },
      BINDING_BACKEND_TOKEN: "token",
      BINDING_BACKEND: {
        async fetch(url) {
          if (url.endsWith("/resolve"))
            return Response.json({
              handle: "operation",
              frame: "callee",
              callerFrame: "caller",
              deadlineMs: 30000,
              target: {
                kind: "worker",
                loaderKey: "account/worker/version",
                workerCodeSha256: "a".repeat(64),
                routeGeneration: 1,
                contentKind: "worker",
              },
            });
          assert.ok(url.endsWith("/connect/finalize"));
          finalizeCalls += 1;
          finalizing.resolve();
          await finish.promise;
          if (outcome === "finalization failure")
            throw new Error("private hop failed");
          return Response.json({ ok: true });
        },
      },
    };
    const completion = new ServiceTransport(ctx, env).connect({
      close: async () => {
        closes += 1;
      },
    });
    assert.deepEqual(
      background,
      [completion],
      "register before the caller can disconnect",
    );
    let settled = false;
    const observed = completion.then(
      () => {
        settled = true;
        return null;
      },
      (error) => {
        settled = true;
        return error;
      },
    );
    await finalizing.promise;
    assert.equal(
      settled,
      false,
      "retained task includes the awaited registry finalization",
    );
    finish.resolve();
    const error = await observed;
    assert.equal(finalizeCalls, outcome === "finalization failure" ? 3 : 1);
    assert.equal(closes, outcome === "disconnect" ? 1 : 0);
    if (outcome === "success") assert.equal(error, null);
    else
      assert.match(
        error.message,
        outcome === "disconnect" ? /SERVICE_UNAVAILABLE/ : /private hop failed/,
      );
  });
}

test("Extension admission injects one host port and no outbound capability", async () => {
  let built;
  const retained = [];
  const hostPort = {};
  const cacheTransport = {
    match() {
      throw new Error("CACHE_UNAVAILABLE");
    },
    put() {},
    delete() {},
    purge() {},
  };
  const ctx = {
    waitUntil(task) {
      retained.push(task);
    },
    props: {
      versionId: "version",
      bindingName: "FILES",
      descriptorSha256: "a".repeat(64),
    },
    exports: {
      ServiceFetchCompletion: ({ props }) => ({ props }),
      ExtensionCacheTransport: ({ props }) => {
        assert.deepEqual(props, {});
        return cacheTransport;
      },
    },
  };
  const env = {
    SYSTEM_COMPATIBILITY_DATE: "2026-09-08",
    SYSTEM_COMPATIBILITY_FLAGS: ["experimental"],
    HOST_EXTENSION_FACTORY: {
      get(identity) {
        assert.equal(identity, "session");
        return hostPort;
      },
    },
    LOADER: {
      get(key, factory) {
        assert.equal(key, "extension/local-files/caller/version/FILES");
        built = factory();
        return {
          getEntrypoint(name, options) {
            assert.equal(name, "__OpenComputeDefaultService");
            assert.deepEqual(options.props.userProps, {
              directory: "invoices",
            });
            return { fetch: async () => new Response("ok") };
          },
        };
      },
    },
    WORKER_LOADER_FACTORY: {
      getEntrypoint(stub, _tails, name, options) {
        return stub.getEntrypoint(name, options);
      },
    },
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: {
      async fetch(url) {
        assert.ok(url.endsWith("/resolve"));
        return Response.json({
          handle: "operation",
          frame: "callee",
          callerFrame: "caller",
          deadlineMs: 30_000,
          target: {
            kind: "extension",
            loaderKey: "extension/local-files/caller/version/FILES",
            mainModule: "facade.js",
            moduleBase64: Buffer.from("export default {};").toString("base64"),
            sessionIdentity: "session",
            props: { directory: "invoices" },
          },
        });
      },
    },
  };
  const request = new Request("https://worker.test/", {
    headers: {
      "x-open-compute-service-frame": JSON.stringify({
        scopeId: "00000000-0000-4000-8000-000000000001",
        parentFrame: null,
      }),
    },
  });
  const response = await new ServiceTransport(ctx, env).fetch(request);
  assert.equal(retained.length, 1);
  assert.equal(await retained[0], response);
  assert.equal(await response.text(), "ok");
  const code = await built;
  assert.equal(code.mainModule, "facade.js");
  assert.equal(code.env.HOST, hostPort);
  assert.deepEqual(Object.keys(code.env), ["HOST"]);
  assert.equal(code.openComputeHostPolicy, true);
  assert.deepEqual(code.openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_POLICY, {
    validation: false,
    entrypointName: undefined,
  });
  const cache = Object.getOwnPropertyDescriptor(
    code.openComputePrivateEnv,
    "__OPEN_COMPUTE_PRIVATE_CACHE",
  );
  assert.equal(cache.enumerable, true);
  assert.equal(cache.value.default, cacheTransport);
  assert.throws(() => cache.value.default.match(), /CACHE_UNAVAILABLE/);
  assert.equal(code.globalOutbound, null);
});

for (const outcome of [
  "rpc",
  "getter",
  "sync constructor failure",
  "async constructor failure",
  "malformed result",
  "locked background stream",
  "handler failure",
]) {
  test(`Service ${outcome} preserves admitted constructor scope and operation cleanup`, async () => {
    const { background } = await import(cloudflare);
    const backgroundStart = background.length;
    const frame = {
      scopeId: crypto.randomUUID(),
      parentFrame: crypto.randomUUID(),
    };
    const admittedFrame = crypto.randomUUID();
    const pending = Promise.withResolvers();
    let completeCalls = 0;
    let constructed = 0;
    const previousWait = scheduler.wait;
    scheduler.wait = async (ms) => {
      if (ms > 1000) await new Promise(() => {});
    };
    const ctx = {
      props: {
        versionId: "version",
        bindingName: "TARGET",
        descriptorSha256: "a".repeat(64),
      },
    };
    const env = {
      LOADER: {
        get() {
          return {
            getEntrypoint(_name, options) {
              constructed += 1;
              assert.deepEqual(options.props, {
                __OPEN_COMPUTE_SERVICE_CONTEXT: {
                  scopeId: frame.scopeId,
                  frame: admittedFrame,
                },
                userProps: { tenant: true },
              });
              const call = (scopeId, child, _reporter, method, args) => {
                assert.equal(scopeId, frame.scopeId);
                assert.equal(child, admittedFrame);
                assert.equal(method, "read");
                if (outcome === "sync constructor failure")
                  throw new Error("constructor failed");
                if (outcome === "async constructor failure")
                  return Promise.reject(new Error("constructor failed"));
                if (outcome === "malformed result") return 42;
                if (outcome === "getter") assert.equal(args, undefined);
                else assert.deepEqual(args, []);
                const background = new ReadableStream({
                  async start(controller) {
                    await pending.promise;
                    controller.close();
                  },
                });
                if (outcome === "locked background stream")
                  background.getReader();
                return {
                  ok: outcome !== "handler failure",
                  value: 42,
                  error: new Error("handler failed"),
                  background,
                };
              };
              return {
                __openComputeServiceRpc: call,
                __openComputeServiceGet: call,
              };
            },
          };
        },
      },
      BINDING_BACKEND_TOKEN: "token",
      BINDING_BACKEND: {
        async fetch(url, init) {
          if (url.endsWith("/resolve")) {
            assert.equal(JSON.parse(init.body).parentFrame, frame.parentFrame);
            return Response.json({
              handle: "operation",
              frame: admittedFrame,
              callerFrame: frame.parentFrame,
              deadlineMs: 30000,
              target: {
                kind: "worker",
                loaderKey: "account/worker/version",
                workerCodeSha256: "a".repeat(64),
                routeGeneration: 1,
                contentKind: "worker",
                props: { tenant: true },
              },
            });
          }
          assert.ok(url.endsWith("/complete"));
          completeCalls += 1;
          return Response.json({ ok: true });
        },
      },
    };
    try {
      const transport = new ServiceTransport(ctx, env);
      const result =
        outcome === "getter"
          ? transport.get(frame, "read")
          : transport.rpc(frame, "read", []);
      if (outcome === "rpc" || outcome === "getter")
        assert.equal(await result, 42);
      else
        await assert.rejects(
          result,
          /constructor failed|handler failed|SERVICE_UNAVAILABLE|locked/,
        );
      assert.equal(constructed, 1);
      const hasBackground = ["rpc", "getter", "handler failure"].includes(
        outcome,
      );
      assert.equal(completeCalls, hasBackground ? 0 : 1);
      pending.resolve();
      await Promise.all(background.slice(backgroundStart));
      assert.equal(completeCalls, 1);
    } finally {
      pending.resolve();
      scheduler.wait = previousWait;
    }
  });
}

const { snapshotControl } = await import(inert);

function deadlineFixture({
  targetOpened = Promise.resolve(),
  rpcError,
  deadlineMs = 25,
  rpcArgs = [],
  rpcValue = "ok",
  retain = async () => ({ retention: crypto.randomUUID() }),
  released = () => {},
} = {}) {
  const background = [];
  const mutations = [];
  const connected = Promise.withResolvers();
  let dispatches = 0;
  let targetCloses = 0;
  const ctx = {
    props: {
      versionId: "version",
      bindingName: "TARGET",
      descriptorSha256: "a".repeat(64),
      entrypoint: "named",
    },
    waitUntil(task) {
      background.push(task);
    },
    exports: { ServiceFetchCompletion: () => ({}) },
  };
  const env = {
    LOADER: {
      get() {
        return {
          getEntrypoint() {
            return {
              async fetch() {
                dispatches++;
                return new Response("ok");
              },
              __openComputeServiceRpc() {
                dispatches++;
                if (rpcError) throw rpcError;
                return {
                  ok: true,
                  value: rpcValue,
                  background: new ReadableStream({
                    start(controller) {
                      controller.close();
                    },
                  }),
                };
              },
              connect() {
                dispatches++;
                connected.resolve();
                return {
                  opened: targetOpened,
                  close: async () => {
                    targetCloses++;
                  },
                };
              },
            };
          },
        };
      },
    },
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: {
      async fetch(url, init) {
        if (url.endsWith("/resolve"))
          return Response.json({
            handle: "operation",
            frame: "callee",
            callerFrame: "caller",
            deadlineMs,
            target: {
              kind: "worker",
              loaderKey: "account/worker/version",
              workerCodeSha256: "a".repeat(64),
              routeGeneration: 1,
              contentKind: "worker",
              entrypoint: "named",
            },
          });
        mutations.push([url, JSON.parse(init.body)]);
        if (url.endsWith("/retain")) return Response.json(await retain());
        if (url.endsWith("/release")) released();
        return Response.json({ ok: true });
      },
    },
  };
  const transport = new ServiceTransport(ctx, env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  let closes = 0;
  return {
    transport,
    frame,
    mutations,
    background,
    dispatches: () => dispatches,
    closes: () => closes,
    targetCloses: () => targetCloses,
    connected: connected.promise,
    invoke(kind) {
      if (kind === "rpc") return transport.rpc(frame, "read", rpcArgs);
      if (kind === "connect")
        return transport.connect({
          close: async () => {
            closes++;
          },
        });
      return transport.fetch(
        new Request("http://service/", {
          headers: { "x-open-compute-service-frame": JSON.stringify(frame) },
        }),
      );
    },
  };
}

function deadlineTimers(t) {
  const timers = [];
  t.mock.method(scheduler, "wait", (delay, options) => {
    const timer = Promise.withResolvers();
    timers.push({ delay, options, ...timer });
    options.signal.addEventListener(
      "abort",
      () => timer.reject(Error("cancelled deadline")),
      { once: true },
    );
    return timer.promise;
  });
  return timers;
}

for (const kind of ["http", "rpc", "connect"]) {
  test(`Service ${kind} releases an admitted operation when target loading stalls`, async (t) => {
    const entered = Promise.withResolvers();
    const loaded = Promise.withResolvers();
    const timers = deadlineTimers(t);
    const previous = snapshotControl.run;
    snapshotControl.run = async () => {
      entered.resolve();
      return loaded.promise;
    };
    t.mock.method(Date, "now", () => 0);
    const fixture = deadlineFixture();
    const outcome = fixture.invoke(kind).then(
      (value) => ({ value }),
      (error) => ({ error }),
    );
    try {
      await entered.promise;
      const activeTimers = timers.filter(
        ({ options }) => !options.signal.aborted,
      );
      assert.equal(
        activeTimers.length,
        1,
        "loading must already be bounded by the admitted deadline",
      );
      const loadingTimer = activeTimers[0];
      assert.equal(loadingTimer.delay, 25);
      loadingTimer.resolve();
      const result = await outcome;
      assert.match(
        result.error?.message ?? "",
        kind === "connect" ? /SERVICE_UNAVAILABLE/ : /SERVICE_TIMEOUT/,
      );
      assert.equal(fixture.dispatches(), 0);
      assert.equal(
        fixture.mutations.filter(([url]) =>
          url.endsWith(kind === "connect" ? "/connect/finalize" : "/complete"),
        ).length,
        1,
      );
      assert.equal(fixture.closes(), kind === "connect" ? 1 : 0);
      assert.equal(loadingTimer.options.signal.aborted, true);
    } finally {
      loaded.resolve({ routeGeneration: 1, contentKind: "worker" });
      await outcome;
      await fixture.transport.completeRoot(fixture.frame.scopeId);
      snapshotControl.run = previous;
    }
    assert.equal(
      fixture.dispatches(),
      0,
      "late snapshot completion cannot dispatch tenant code",
    );
  });

  test(`Service ${kind} cannot start tenant work after loading consumes its deadline`, async (t) => {
    let now = 0;
    t.mock.method(Date, "now", () => now);
    t.mock.method(scheduler, "wait", () => new Promise(() => {}));
    const previous = snapshotControl.run;
    snapshotControl.run = async () => {
      now = 25;
      return { routeGeneration: 1, contentKind: "worker" };
    };
    const fixture = deadlineFixture();
    try {
      await assert.rejects(
        fixture.invoke(kind),
        kind === "connect" ? /SERVICE_UNAVAILABLE/ : /SERVICE_TIMEOUT/,
      );
      assert.equal(
        fixture.dispatches(),
        0,
        "check budget before invoking the tenant method",
      );
    } finally {
      await fixture.transport.completeRoot(fixture.frame.scopeId);
      snapshotControl.run = previous;
    }
  });

  test(`Service ${kind} dispatch uses only the remaining budget after target loading`, async (t) => {
    let now = 0;
    t.mock.method(Date, "now", () => now);
    const timers = deadlineTimers(t);
    const previous = snapshotControl.run;
    snapshotControl.run = async () => {
      now += 5;
      return { routeGeneration: 1, contentKind: "worker" };
    };
    const fixture = deadlineFixture();
    try {
      await fixture.invoke(kind);
      assert.equal(fixture.dispatches(), 1);
      assert.ok(now > 0 && now < 25);
      assert.equal(timers.at(-1).delay, 25 - now);
      assert.ok(timers.every(({ options }) => options.signal.aborted));
    } finally {
      snapshotControl.run = previous;
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });

  test(`Service ${kind} cancels all startup timers after successful dispatch`, async (t) => {
    t.mock.method(Date, "now", () => 0);
    const timers = deadlineTimers(t);
    const fixture = deadlineFixture();
    try {
      await fixture.invoke(kind);
      assert.equal(fixture.dispatches(), 1);
      assert.ok(timers.length >= 2, "loading and dispatch share the deadline");
      assert.ok(timers.every(({ options }) => options.signal.aborted));
      assert.ok(timers.every(({ delay }) => delay === 25));
    } finally {
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });

  test(`Service ${kind} finalizes an admission with an invalid deadline`, async (t) => {
    t.mock.method(Date, "now", () => 0);
    const timers = deadlineTimers(t);
    const fixture = deadlineFixture({ deadlineMs: 0 });
    try {
      await assert.rejects(fixture.invoke(kind), /SERVICE_UNAVAILABLE/);
      assert.equal(fixture.dispatches(), 0);
      assert.equal(timers.length, 0);
      assert.equal(
        fixture.mutations.filter(([url]) =>
          url.endsWith(kind === "connect" ? "/connect/finalize" : "/complete"),
        ).length,
        1,
      );
    } finally {
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });
}

test("Service RPC cancels its deadline after a synchronous tenant invocation failure", async (t) => {
  t.mock.method(Date, "now", () => 0);
  const timers = deadlineTimers(t);
  const fixture = deadlineFixture({
    rpcError: Error("tenant invocation failed"),
  });
  try {
    await assert.rejects(fixture.invoke("rpc"), /tenant invocation failed/);
    assert.equal(fixture.dispatches(), 1);
    assert.ok(timers.every(({ options }) => options.signal.aborted));
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
      1,
    );
  } finally {
    await fixture.transport.completeRoot(fixture.frame.scopeId);
  }
});

test("Service CONNECT closes both sockets when the target never opens", async (t) => {
  t.mock.method(Date, "now", () => 0);
  const timers = deadlineTimers(t);
  const opened = Promise.withResolvers();
  const fixture = deadlineFixture({ targetOpened: opened.promise });
  const previous = tunnelControl.run;
  let tunnels = 0;
  tunnelControl.run = async () => {
    tunnels++;
  };
  const outcome = fixture.invoke("connect").then(
    () => null,
    (error) => error,
  );
  try {
    // Observe connect() creating the target without allowing its opened promise to settle.
    await fixture.connected;
    const activeTimers = timers.filter(
      ({ options }) => !options.signal.aborted,
    );
    assert.equal(activeTimers.length, 1);
    activeTimers[0].resolve();
    assert.match((await outcome).message, /SERVICE_UNAVAILABLE/);
    assert.equal(fixture.closes(), 1);
    assert.equal(fixture.targetCloses(), 1);
    assert.equal(tunnels, 0);
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/connect/finalize"))
        .length,
      1,
    );
    assert.ok(timers.every(({ options }) => options.signal.aborted));
  } finally {
    opened.resolve();
    await outcome;
    tunnelControl.run = previous;
  }
  assert.equal(tunnels, 0, "late opening cannot start a finalized tunnel");
});

test("Service CONNECT leaves an established tunnel alive beyond its startup deadline", async (t) => {
  let now = 0;
  t.mock.method(Date, "now", () => now);
  const timers = deadlineTimers(t);
  const entered = Promise.withResolvers();
  const finish = Promise.withResolvers();
  const previous = tunnelControl.run;
  tunnelControl.run = async () => {
    entered.resolve();
    await finish.promise;
  };
  const fixture = deadlineFixture();
  let settled = false;
  const outcome = fixture.invoke("connect").then(() => {
    settled = true;
  });
  try {
    await entered.promise;
    now = 100;
    assert.ok(timers.every(({ options }) => options.signal.aborted));
    assert.equal(settled, false);
    assert.equal(fixture.mutations.length, 0);
    assert.equal(fixture.closes(), 0);
    assert.equal(fixture.targetCloses(), 0);
    finish.resolve();
    await outcome;
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/connect/finalize"))
        .length,
      1,
    );
  } finally {
    finish.resolve();
    await outcome;
    tunnelControl.run = previous;
  }
});

test("private retention receipt extends its owner and disposal releases abandoned ownership once", async () => {
  const { background } = await import(cloudflare);
  const backgroundStart = background.length;
  const entered = Promise.withResolvers();
  const receipt = Promise.withResolvers();
  const fixture = deadlineFixture({
    async retain() {
      entered.resolve();
      return await receipt.promise;
    },
  });
  const acquisition = fixture.transport.retainCapability(
    crypto.randomUUID(),
    "target",
    Date.now() + 1000,
  );
  await entered.promise;
  assert.equal(background.length, backgroundStart + 1);
  receipt.resolve({ retention: crypto.randomUUID() });
  const controller = await acquisition;
  controller[Symbol.dispose]();
  await Promise.all(background.slice(backgroundStart));
  await controller.release();
  assert.equal(
    fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
    1,
  );
});

test("private receipt producer releases expired ownership before exporting a native controller", async (t) => {
  let now = 0;
  t.mock.method(Date, "now", () => now);
  const entered = Promise.withResolvers();
  const receipt = Promise.withResolvers();
  const fixture = deadlineFixture({
    async retain() {
      entered.resolve();
      return await receipt.promise;
    },
  });
  let exported;
  const observed = fixture.transport
    .retainCapability(crypto.randomUUID(), "target", 10)
    .then(
      (value) => {
        exported = value;
      },
      (error) => error,
    );
  await entered.promise;
  now = 10;
  receipt.resolve({ retention: crypto.randomUUID() });
  const error = await observed;
  try {
    assert.equal(
      error?.message,
      "SERVICE_TIMEOUT",
      "producer must reject an expired receipt before native export",
    );
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
      1,
    );
  } finally {
    await exported?.release();
  }
});

test("private receipt producer rejects malformed and expired deadlines before authority writes", async (t) => {
  t.mock.method(Date, "now", () => 0);
  const fixture = deadlineFixture();
  for (const deadlineAt of [undefined, NaN, Infinity, 30_001]) {
    await assert.rejects(
      fixture.transport.retainCapability(
        crypto.randomUUID(),
        "target",
        deadlineAt,
      ),
      /SERVICE_UNAVAILABLE/,
    );
  }
  for (const deadlineAt of [0, -1]) {
    await assert.rejects(
      fixture.transport.retainCapability(
        crypto.randomUUID(),
        "target",
        deadlineAt,
      ),
      /SERVICE_TIMEOUT/,
    );
  }
  assert.equal(fixture.mutations.length, 0);
});

test("private receipt producer sanitizes failure to release expired ownership", async (t) => {
  let now = 0;
  t.mock.method(Date, "now", () => now);
  const fixture = deadlineFixture({
    async retain() {
      now = 10;
      return { retention: crypto.randomUUID() };
    },
    released() {
      throw Error("private release detail");
    },
  });
  await assert.rejects(
    fixture.transport.retainCapability(crypto.randomUUID(), "target", 10),
    (error) => {
      assert.equal(error.message, "SERVICE_UNAVAILABLE");
      return true;
    },
  );
  assert.equal(
    fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
    1,
  );
});

for (const owner of ["caller", "target"]) {
  test(`Service preserves successful ${owner} capability ownership and retains repeated aliases once`, async (t) => {
    t.mock.method(Date, "now", () => 0);
    const timers = deadlineTimers(t);
    let controller;
    let activations = 0;
    const capability = {
      __openComputeServiceCapability: 1,
      kind: "function",
      handle: {
        activate(retained) {
          activations++;
          controller = retained;
        },
      },
    };
    const value = { first: capability, again: capability };
    const fixture = deadlineFixture({
      rpcArgs: owner === "caller" ? [value] : [],
      rpcValue: owner === "target" ? value : "ok",
    });
    try {
      assert.equal(
        await fixture.invoke("rpc"),
        owner === "target" ? value : "ok",
      );
      assert.equal(activations, 1);
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/retain")).length,
        1,
      );
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
        0,
      );
      assert.deepEqual(await controller.begin(fixture.frame), { ok: true });
      await controller.release();
      assert.throws(
        () => controller.begin(fixture.frame),
        /SERVICE_BINDING_DENIED/,
      );
      await controller.complete(crypto.randomUUID());
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
        2,
        "admitted operation cleanup remains available after capability release",
      );
      assert.throws(
        () => controller.complete("invalid"),
        /SERVICE_BINDING_DENIED/,
      );
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
        1,
      );
      assert.ok(timers.every(({ options }) => options.signal.aborted));
    } finally {
      await controller?.release();
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });

  test(
    `Service releases a late ${owner} retention without activating its expired capability`,
    { timeout: 1000 },
    async (t) => {
      let now = 0;
      t.mock.method(Date, "now", () => now);
      const timers = deadlineTimers(t);
      const entered = Promise.withResolvers();
      const finish = Promise.withResolvers();
      const handedOff = Promise.withResolvers();
      let activations = 0;
      const capability = {
        __openComputeServiceCapability: 1,
        kind: "function",
        handle: {
          activate() {
            activations++;
            handedOff.resolve();
          },
        },
      };
      const fixture = deadlineFixture({
        rpcArgs: owner === "caller" ? [capability] : [],
        rpcValue: owner === "target" ? capability : "ok",
        retain: async () => {
          entered.resolve();
          await finish.promise;
          return { retention: crypto.randomUUID() };
        },
        released: () => handedOff.resolve(),
      });
      const outcome = fixture.invoke("rpc").then(
        (value) => ({ value }),
        (error) => ({ error }),
      );
      try {
        await entered.promise;
        const activeTimers = timers.filter(
          ({ options }) => !options.signal.aborted,
        );
        assert.equal(
          activeTimers.length,
          1,
          "capability handoff must retain the admitted deadline",
        );
        now = 25;
        activeTimers[0].resolve();
        assert.match((await outcome).error?.message ?? "", /SERVICE_TIMEOUT/);
        assert.equal(fixture.dispatches(), owner === "caller" ? 0 : 1);
        finish.resolve();
        await handedOff.promise;
        assert.equal(activations, 0);
        assert.equal(
          fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
          1,
        );
        assert.equal(
          fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
          1,
        );
        assert.ok(timers.every(({ options }) => options.signal.aborted));
      } finally {
        finish.resolve();
        await outcome;
        await fixture.transport.completeRoot(fixture.frame.scopeId);
      }
    },
  );

  test(`Service releases ${owner} retentions after activation fails`, async (t) => {
    t.mock.method(Date, "now", () => 0);
    const timers = deadlineTimers(t);
    const capability = {
      __openComputeServiceCapability: 1,
      kind: "function",
      handle: {
        activate() {
          throw Error("activation failed");
        },
      },
    };
    const fixture = deadlineFixture({
      rpcArgs: owner === "caller" ? [capability] : [],
      rpcValue: owner === "target" ? capability : "ok",
    });
    try {
      await assert.rejects(fixture.invoke("rpc"), /activation failed/);
      assert.equal(fixture.dispatches(), owner === "caller" ? 0 : 1);
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
        1,
      );
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
        1,
      );
      assert.ok(timers.every(({ options }) => options.signal.aborted));
    } finally {
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });

  test(`Service revokes ${owner} retention while capability activation is still pending`, async (t) => {
    let now = 0;
    t.mock.method(Date, "now", () => now);
    const timers = deadlineTimers(t);
    const entered = Promise.withResolvers();
    const finish = Promise.withResolvers();
    let controller;
    const capability = {
      __openComputeServiceCapability: 1,
      kind: "function",
      handle: {
        async activate(retained) {
          controller = retained;
          entered.resolve();
          await finish.promise;
        },
      },
    };
    const fixture = deadlineFixture({
      rpcArgs: owner === "caller" ? [capability] : [],
      rpcValue: owner === "target" ? capability : "ok",
    });
    const outcome = fixture.invoke("rpc").then(
      (value) => ({ value }),
      (error) => ({ error }),
    );
    try {
      await entered.promise;
      const activeTimers = timers.filter(
        ({ options }) => !options.signal.aborted,
      );
      assert.equal(activeTimers.length, 1);
      now = 25;
      activeTimers[0].resolve();
      assert.match((await outcome).error?.message ?? "", /SERVICE_TIMEOUT/);
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
        1,
      );
      assert.throws(
        () => controller.begin(fixture.frame),
        /SERVICE_BINDING_DENIED/,
      );
      finish.resolve();
      assert.equal(fixture.dispatches(), owner === "caller" ? 0 : 1);
      assert.ok(timers.every(({ options }) => options.signal.aborted));
    } finally {
      finish.resolve();
      await outcome;
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });

  test(`Service revokes the ${owner} handoff batch and releases a late second receipt`, async (t) => {
    let now = 0;
    t.mock.method(Date, "now", () => now);
    const timers = deadlineTimers(t);
    const entered = Promise.withResolvers();
    const finish = Promise.withResolvers();
    const releasedAll = Promise.withResolvers();
    let firstController;
    let lateActivations = 0;
    let retained = 0;
    let releases = 0;
    const capabilities = [
      {
        __openComputeServiceCapability: 1,
        kind: "function",
        handle: {
          activate(controller) {
            firstController = controller;
          },
        },
      },
      {
        __openComputeServiceCapability: 1,
        kind: "function",
        handle: {
          activate() {
            lateActivations++;
          },
        },
      },
    ];
    const fixture = deadlineFixture({
      rpcArgs: owner === "caller" ? capabilities : [],
      rpcValue: owner === "target" ? capabilities : "ok",
      retain: async () => {
        retained++;
        if (retained === 2) {
          entered.resolve();
          await finish.promise;
        }
        return { retention: crypto.randomUUID() };
      },
      released: () => {
        if (++releases === 2) releasedAll.resolve();
      },
    });
    const outcome = fixture.invoke("rpc").then(
      (value) => ({ value }),
      (error) => ({ error }),
    );
    try {
      await entered.promise;
      const activeTimers = timers.filter(
        ({ options }) => !options.signal.aborted,
      );
      assert.equal(activeTimers.length, 1);
      now = 25;
      activeTimers[0].resolve();
      assert.match((await outcome).error?.message ?? "", /SERVICE_TIMEOUT/);
      assert.equal(
        releases,
        1,
        "revoke the first receipt before waiting for the second",
      );
      assert.throws(
        () => firstController.begin(fixture.frame),
        /SERVICE_BINDING_DENIED/,
      );
      finish.resolve();
      await releasedAll.promise;
      assert.equal(lateActivations, 0);
      assert.equal(retained, 2);
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/release")).length,
        2,
      );
      assert.equal(
        fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
        1,
      );
    } finally {
      finish.resolve();
      await outcome;
      await fixture.transport.completeRoot(fixture.frame.scopeId);
    }
  });
}

test("Service still completes its operation when retention cleanup fails and sanitizes the failure", async (t) => {
  t.mock.method(Date, "now", () => 0);
  const timers = deadlineTimers(t);
  const capability = {
    __openComputeServiceCapability: 1,
    kind: "function",
    handle: {
      activate() {
        throw Error("activation failed");
      },
    },
  };
  const fixture = deadlineFixture({
    rpcArgs: [capability],
    released: () => {
      throw Error("private backend detail");
    },
  });
  try {
    await assert.rejects(fixture.invoke("rpc"), (error) => {
      assert.equal(error.message, "SERVICE_UNAVAILABLE");
      return true;
    });
    assert.equal(fixture.dispatches(), 0);
    assert.equal(
      fixture.mutations.filter(([url]) => url.endsWith("/complete")).length,
      1,
    );
    assert.ok(timers.every(({ options }) => options.signal.aborted));
  } finally {
    await fixture.transport.completeRoot(fixture.frame.scopeId);
  }
});

test("Service transport dispatches arbitrary native method and getter property names", async (t) => {
  t.mock.method(scheduler, "wait", () => new Promise(() => {}));
  const fixture = delayedAdmission();
  const calls = [];
  fixture.env.LOADER.get = () => ({
    getEntrypoint: () => ({
      __openComputeServiceRpc(_scope, _frame, _reporter, name, args) {
        calls.push(["call", name, args]);
        return {
          ok: true,
          value: 22,
          background: new ReadableStream({
            start(c) {
              c.close();
            },
          }),
        };
      },
      __openComputeServiceGet(_scope, _frame, _reporter, name) {
        calls.push(["get", name]);
        return {
          ok: true,
          value: 20,
          background: new ReadableStream({
            start(c) {
              c.close();
            },
          }),
        };
      },
    }),
  });
  const transport = new ServiceTransport(fixture.ctx, fixture.env);
  const frame = { scopeId: crypto.randomUUID(), parentFrame: null };
  fixture.firstAdmission.resolve();
  try {
    for (const name of [
      "中文",
      "with-hyphen",
      "",
      "x".repeat(129),
      "prototype",
    ]) {
      assert.equal(await transport.rpc(frame, name, [2]), 22);
      assert.equal(await transport.get(frame, name), 20);
      assert.deepEqual(calls.splice(0), [
        ["call", name, [2]],
        ["get", name],
      ]);
    }
  } finally {
    await transport.completeRoot(frame.scopeId);
  }
});

import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  export class RpcTarget {}
  export class WorkerEntrypoint {
    constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  }
  export function waitUntil() {}
`);
const inert = moduleUrl(`
  export const routeDefaultHttp = () => "worker";
  export const tenantEnv = () => ({});
  export const modulesFor = () => ({});
  export const inboundSocketTargetAddress = async () => "example.invalid:443";
  export const tunnelControl = { run: async () => {} };
  export const tunnelSockets = (...args) => tunnelControl.run(...args);
  export const assembleOnce = async (_key, factory) => factory();
  export const bindingError = code => new Error(code);
  export const BINDING_TOKEN_HEADER = "x-binding-token";
  export const currentStartupGeneration = () => "generation";
  export const collectableWorkerCode = value => value;
  export const doPolicy = () => ({});
  export const INTERNAL_HEADERS = [];
  export const lockWorkerCode = () => ({});
  export const resolveSnapshot = async () => ({ routeGeneration: 1, contentKind: "worker" });
  export const tenantGlobalOutbound = () => ({});
`);

globalThis.scheduler = { wait: async () => {} };

const transportUrl = moduleUrl(await compileRuntime("services/transport.ts", {
  "cloudflare:workers": cloudflare,
  "../assets/router.js": inert,
  "../loader/bindings.js": inert,
  "../loader/modules.js": inert,
  "../observability/collector.js": inert,
  "../sockets/tunnel.js": inert,
  "../loader/shared.js": inert,
}));
const { ServiceTransport, retryServiceControl } = await import(transportUrl);

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
    await retryServiceControl(env, "/internal/services/v1/complete", { handle: "operation" }),
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
      props: { versionId: "version", bindingName: "TARGET", descriptorSha256: "a".repeat(64) },
      waitUntil(task) { background.push(task); },
    };
    const env = {
      LOADER: { get() { return { getEntrypoint() {
        return { connect() { return { opened: Promise.resolve() }; } };
      } }; } },
      BINDING_BACKEND_TOKEN: "token",
      BINDING_BACKEND: { async fetch(url) {
        if (url.endsWith("/resolve")) return Response.json({
          handle: "operation", frame: "callee", callerFrame: "caller", deadlineMs: 30000,
          target: { loaderKey: "account/worker/version", workerCodeSha256: "a".repeat(64),
            routeGeneration: 1, contentKind: "worker" },
        });
        assert.ok(url.endsWith("/connect/finalize"));
        finalizeCalls += 1;
        finalizing.resolve();
        await finish.promise;
        if (outcome === "finalization failure") throw new Error("private hop failed");
        return Response.json({ ok: true });
      } },
    };
    const completion = new ServiceTransport(ctx, env).connect({
      close: async () => { closes += 1; },
    });
    assert.deepEqual(background, [completion], "register before the caller can disconnect");
    let settled = false;
    const observed = completion.then(
      () => { settled = true; return null; },
      error => { settled = true; return error; },
    );
    await finalizing.promise;
    assert.equal(settled, false, "retained task includes the awaited registry finalization");
    finish.resolve();
    const error = await observed;
    assert.equal(finalizeCalls, outcome === "finalization failure" ? 3 : 1);
    assert.equal(closes, outcome === "disconnect" ? 1 : 0);
    if (outcome === "success") assert.equal(error, null);
    else assert.match(error.message,
      outcome === "disconnect" ? /SERVICE_UNAVAILABLE/ : /private hop failed/);
  });
}

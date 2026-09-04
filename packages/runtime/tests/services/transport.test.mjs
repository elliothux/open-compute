import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  export class RpcTarget {}
  export class WorkerEntrypoint {}
  export function waitUntil() {}
`);
const inert = moduleUrl(`
  export const routeDefaultHttp = () => "worker";
  export const tenantEnv = () => ({});
  export const modulesFor = () => ({});
  export const inboundSocketTargetAddress = async () => "example.invalid:443";
  export const tunnelSockets = async () => {};
  export const assembleOnce = async (_key, factory) => factory();
  export const bindingError = code => new Error(code);
  export const BINDING_TOKEN_HEADER = "x-binding-token";
  export const currentStartupGeneration = () => "generation";
  export const collectableWorkerCode = value => value;
  export const doPolicy = () => ({});
  export const INTERNAL_HEADERS = [];
  export const lockWorkerCode = () => ({});
  export const resolveSnapshot = async () => ({});
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
const { retryServiceControl } = await import(transportUrl);

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

import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const imports = {
  "../assets/router.js": moduleUrl(
    "export function routeDefaultHttp() { throw Error('unused'); }",
  ),
  "../observability/collector.js": moduleUrl(
    "export function observedEntrypoint(stub) { return stub.getEntrypoint(); }",
  ),
  "../queues/metrics.js": moduleUrl(await compileRuntime("queues/metrics.ts")),
  "../services/facade.js": moduleUrl(
    "export const SERVICE_WEBSOCKET_HANDOFF_HEADER='x-handoff'; export function serviceWebSocketHandoffHandles() { throw Error('unused'); }",
  ),
  "./bindings.js": moduleUrl(
    "export function tenantEnv() { throw Error('unused'); } export function validationEnv() { throw Error('unused'); }",
  ),
  "./envelope.js": moduleUrl(await compileRuntime("loader/envelope.ts")),
  "./modules.js": moduleUrl(
    "export function modulesFor() { throw Error('unused'); }",
  ),
  "./shared.js":
    moduleUrl(`export const INTERNAL_HEADERS=[],TOKEN_HEADER='x-open-compute-internal-token';
    export const bindingError=(code)=>Object.assign(new Error(code),{stableCode:code});
    export const stableCode=(error)=>error?.stableCode;
    export const isRecord=(value)=>value!==null && typeof value==='object' && !Array.isArray(value);
    export async function resolveSnapshot(env) { ++env.reads; return env.snapshot; }
    export function doPolicy() { throw Error('unused'); }
    export function assembleOnce() { throw Error('unused'); }
    export function snapshotWorkerCode() { throw Error('unused'); }
    export function tenantGlobalOutbound() { throw Error('unused'); }
    export function tenantLoader(env) { return env.LOADER; }
  export async function loadTenantStub(env, _loaderKey, name, code) {
    return { stub: env.LOADER.get(name, code), release() {} };
  }`),
};
const { handleScheduled } = await importRuntime("loader/dispatch.ts", imports);
const instance = "019c0000000070008000000000000001";
const key = `${instance}/019c0000-0000-7000-8000-000000000002/019c0000-0000-7000-8000-000000000003`;
const target = {
  cron: "0 * * * *",
  scheduledHandler: true,
  workflowBindings: [],
};
const payload = () => ({ ...target, scheduledTimeMs: 60_000 });
function fixture(
  scheduled = async () => ({ outcome: "ok", noRetry: false }),
  targets = [target],
) {
  const env = {
    reads: 0,
    loads: 0,
    calls: [],
    snapshot: { scheduledTargets: targets },
    LOADER: {
      get() {
        ++env.loads;
        return {
          getEntrypoint() {
            return {
              async scheduled(...args) {
                env.calls.push(args);
                return scheduled(...args);
              },
            };
          },
        };
      },
    },
  };
  const raw = (body) =>
    handleScheduled(
      new Request("https://loader.invalid/scheduled", {
        method: "POST",
        headers: {
          "x-open-compute-loader-key": key,
          "x-open-compute-instance-id": instance,
          "x-open-compute-worker-code-sha256": "a".repeat(64),
          "x-open-compute-route-generation": "1",
        },
        body,
      }),
      env,
      {},
    );
  return { env, raw, dispatch: (value) => raw(JSON.stringify(value)) };
}
test("scheduled dispatch restores the native Date and preserves handler/workflow-only outcomes", async () => {
  for (const declaration of [
    target,
    { ...target, scheduledHandler: false, workflowBindings: ["FLOW"] },
  ]) {
    const result = { outcome: "ok", noRetry: true };
    const { env, dispatch } = fixture(async () => result, [declaration]);
    const response = await dispatch({
      ...declaration,
      scheduledTimeMs: 60_000,
    });
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), result);
    assert.equal(env.reads, 1);
    assert.equal(env.loads, 1);
    assert.equal(env.calls.length, 1);
    assert.deepEqual(env.calls[0], [
      { cron: target.cron, scheduledTime: new Date(60_000) },
    ]);
  }
});
test("scheduled dispatch rejects malformed request JSON before resolving authority", async () => {
  const { env, raw } = fixture();
  const response = await raw("{");
  assert.equal(response.status, 422);
  assert.equal((await response.json()).error.code, "CRON_EXPRESSION_INVALID");
  assert.equal(env.reads, 0);
  assert.equal(env.loads, 0);
  assert.equal(env.calls.length, 0);
});
test("scheduled dispatch rejects invalid time and workflow frames before loading", async () => {
  for (const value of [
    null,
    [],
    { ...payload(), scheduledTimeMs: -60_000 },
    { ...payload(), scheduledTimeMs: 1 },
    {
      ...payload(),
      scheduledTimeMs: Math.floor(Number.MAX_SAFE_INTEGER / 60_000) * 60_000,
    },
    { ...payload(), scheduledHandler: false },
    { ...payload(), workflowBindings: ["FLOW", "FLOW"] },
    { ...payload(), workflowBindings: ["__internal"] },
    { ...payload(), workflowBindings: ["OPEN_COMPUTE_PRIVATE"] },
    { ...payload(), workflowBindings: ["Z", "A"] },
    { ...payload(), cron: "" },
  ]) {
    const { env, dispatch } = fixture();
    const response = await dispatch(value);
    assert.equal(response.status, 422);
    assert.equal((await response.json()).error.code, "CRON_EXPRESSION_INVALID");
    assert.equal(env.reads, 0);
    assert.equal(env.loads, 0);
    assert.equal(env.calls.length, 0);
  }
});
test("scheduled dispatch rechecks the persisted activation before invoking tenant code", async () => {
  for (const value of [
    { ...payload(), cron: "1 * * * *" },
    { ...payload(), scheduledHandler: false, workflowBindings: ["FLOW"] },
    { ...payload(), workflowBindings: ["FLOW"] },
  ]) {
    const { env, dispatch } = fixture();
    const response = await dispatch(value);
    assert.equal(response.status, 422);
    assert.equal((await response.json()).error.code, "CRON_ACTIVATION_STALE");
    assert.equal(env.reads, 1);
    assert.equal(env.calls.length, 0);
  }
});
test("scheduled dispatch sanitizes thrown runtime diagnostics and preserves native failure results", async () => {
  const { dispatch } = fixture(async () => {
    throw Error("Traceback /session/private.py private-secret");
  });
  const response = await dispatch(payload());
  assert.equal(response.status, 500);
  const body = await response.text();
  assert.ok(!body.includes("private"));
  assert.equal(JSON.parse(body).error.code, "CRON_CUSTOM_EVENT_UNSUPPORTED");
  const result = { outcome: "exception", noRetry: true };
  const native = fixture(async () => result);
  assert.deepEqual(await (await native.dispatch(payload())).json(), result);
});

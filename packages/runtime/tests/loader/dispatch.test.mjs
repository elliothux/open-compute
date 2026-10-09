import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const { assertEnvelope } = await importRuntime("loader/envelope.ts");

const instanceId = "019c0000000070008000000000000001";
const workerId = "019c0000-0000-7000-8000-000000000002";
const versionId = "019c0000-0000-7000-8000-000000000003";

function request(key, headerId = instanceId) {
  return new Request("https://worker.invalid/", {
    headers: {
      "x-open-compute-loader-key": key,
      "x-open-compute-instance-id": headerId,
      "x-open-compute-worker-code-sha256": "a".repeat(64),
      "x-open-compute-route-generation": "1",
    },
  });
}

test("dispatch accepts only the current scoped loader identity", () => {
  const key = `${instanceId}/${workerId}/${versionId}`;
  assert.equal(assertEnvelope(request(key), false).loaderKey, key);
  for (const [candidate, headerId] of [
    [key, "019c0000000070008000000000000004"],
    [`${workerId}/${workerId}/${versionId}`, instanceId],
    [`${instanceId}/${workerId}/${versionId}/extra`, instanceId],
  ]) {
    assert.throws(() => assertEnvelope(request(candidate, headerId), false));
  }
});

const shared = moduleUrl(
  await compileRuntime("loader/shared.ts", {
    "./snapshot.js": moduleUrl("export function assertSnapshot() {}"),
    "./python-snapshot.js": moduleUrl(
      "export function resolvePythonSnapshot() { throw Error('unused'); }",
    ),
  }),
);
const { handleDispatch, validateDurableObjectClass } = await importRuntime(
  "loader/dispatch.ts",
  {
    "../assets/router.js": moduleUrl(
      "export function routeDefaultHttp() { return 'worker'; }",
    ),
    "../observability/collector.js": moduleUrl(
      "export function observedEntrypoint(stub) { return stub.getEntrypoint(); }",
    ),
    "../services/facade.js": moduleUrl(
      "export const SERVICE_WEBSOCKET_HANDOFF_HEADER='x-handoff'; export function serviceWebSocketHandoffHandles() { return []; }",
    ),
    "./bindings.js": moduleUrl(
      "export function tenantEnv() { throw Error('unused'); } export function validationEnv() { throw Error('unused'); }",
    ),
    "./envelope.js": moduleUrl(await compileRuntime("loader/envelope.ts")),
    "../queues/metrics.js": moduleUrl(
      await compileRuntime("queues/metrics.ts"),
    ),
    "./modules.js": moduleUrl(
      "export function modulesFor() { throw Error('unused'); }",
    ),
    "./shared.js": shared,
  },
);

test("runtime exceptions containing bundle vocabulary stay sanitized runtime failures", async () => {
  const key = `${instanceId}/${workerId}/${versionId}`;
  const dispatch = (fetch, validation = false) =>
    handleDispatch(
      request(key),
      {
        RUNTIME_SOURCE: {
          fetch: async () =>
            Response.json({
              loaderKey: key,
              workerCodeSha256: "a".repeat(64),
              routeGeneration: 1,
              contentKind: "worker",
            }),
        },
        // This test exercises an already-loaded deployment; assembly has separate coverage.
        LOADER: {
          get() {
            return {
              getEntrypoint() {
                return { fetch };
              },
            };
          },
        },
      },
      {},
      validation,
    );
  for (const message of [
    "Traceback: /session/metadata/python_modules/workers/asgi.py\nRuntimeError: private-secret",
    "unexpected syntax in application input: private-secret",
    "parse module wasm initialization startup: private-secret",
  ]) {
    const fail = async () => {
      throw new Error(message);
    };
    const response = await dispatch(fail);
    assert.equal(response.status, 500);
    assert.equal(response.headers.get("x-open-compute-execution-started"), "1");
    const payload = await response.json();
    assert.equal(payload.error.code, "RUNTIME_INTERNAL");
    assert.doesNotMatch(
      JSON.stringify(payload),
      /private-secret|Traceback|session|asgi/,
    );
    const invalid = await dispatch(fail, true);
    assert.equal(invalid.status, 422);
    assert.equal((await invalid.json()).error.code, "BUNDLE_RUNTIME_INVALID");
  }
  const good = await dispatch(async () => new Response("ready"));
  assert.equal(good.status, 200);
  assert.equal(await good.text(), "ready");
  const validated = await dispatch(
    async () => new Response("open-compute-validation-v1"),
    true,
  );
  assert.equal(validated.status, 204);
  for (const validation of [false, true]) {
    const unavailable = await dispatch(async () => {
      throw Object.assign(new Error("private-secret"), {
        stableCode: "ARTIFACT_UNAVAILABLE",
      });
    }, validation);
    assert.equal(unavailable.status, 503);
    assert.equal((await unavailable.json()).error.code, "ARTIFACT_UNAVAILABLE");
    const limited = await dispatch(async () => {
      throw new Error("CPU time limit exceeded");
    }, validation);
    assert.equal(limited.status, validation ? 422 : 500);
    const error = (await limited.json()).error;
    assert.equal(
      error.code,
      validation ? "BUNDLE_RUNTIME_INVALID" : "RESOURCE_LIMIT_EXCEEDED",
    );
    assert.equal(error.cloudflareCode, validation ? 10021 : 1102);
  }
  for (const [code, status] of [
    ["SERVICE_BINDING_DENIED", 403],
    ["SERVICE_TARGET_NOT_READY", 503],
    ["SERVICE_UNAVAILABLE", 503],
    ["SERVICE_ENTRYPOINT_NOT_FOUND", 404],
    ["SERVICE_LIMIT_EXCEEDED", 429],
    ["SERVICE_TIMEOUT", 504],
    ["DO_DISPATCH_TIMEOUT", 504],
    ["DO_STORAGE_LIMIT", 429],
    ["DO_STORAGE_UNAVAILABLE", 503],
    ["DO_RUNTIME_EXCEPTION", 500],
    ["WORKFLOW_RUNTIME_UNAVAILABLE", 503],
    ["WORKFLOW_VERSION_NOT_READY", 503],
    ["WORKFLOW_BINDING_STALE", 409],
  ]) {
    // JS RPC can retain own error properties; a Python exception can retain
    // only the native message. Both carriers must expose the same contract.
    for (const failure of [
      new Error(`remote ${code}: private-secret`),
      Object.assign(new Error("private-secret"), { stableCode: code }),
    ]) {
      const response = await dispatch(async () => {
        throw failure;
      });
      assert.equal(response.status, status);
      const payload = await response.json();
      assert.equal(payload.error.code, code);
      assert.doesNotMatch(JSON.stringify(payload), /private-secret|remote/);
    }
  }
});

test("DO admission awaits loading and rejects failed or malformed host validation", async () => {
  const key = `${instanceId}/${workerId}/${versionId}`;
  const invoke = (fetch) => {
    const input = request(key);
    input.headers.set("x-open-compute-entrypoint", "Named");
    return validateDurableObjectClass(input, {
      RUNTIME_SOURCE: {
        fetch: async () =>
          Response.json({
            loaderKey: key,
            workerCodeSha256: "a".repeat(64),
            routeGeneration: 1,
            contentKind: "worker",
          }),
      },
      LOADER: {
        get() {
          return {
            getDurableObjectClass() {
              assert.fail("a lazy class handle is not admission proof");
            },
            getEntrypoint() {
              return { fetch };
            },
          };
        },
      },
    });
  };
  const load = Promise.withResolvers();
  let settled = false;
  const pending = invoke(() => load.promise).then((response) => {
    settled = true;
    return response;
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(settled, false);
  load.resolve(new Response("open-compute-validation-v1"));
  assert.equal((await pending).status, 204);
  for (const fetch of [
    async () => {
      throw Error("private tenant class/source/secret");
    },
    async () => new Response("wrong nonce"),
    async () => new Response("open-compute-validation-v1", { status: 500 }),
  ]) {
    const response = await invoke(fetch);
    assert.equal(response.status, 422);
    const body = await response.text();
    assert.equal(JSON.parse(body).error.code, "DO_CLASS_NOT_FOUND");
    assert.doesNotMatch(body, /private tenant|secret|wrong nonce/);
  }
});

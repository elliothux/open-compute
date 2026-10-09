import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const leaf = async (name) => moduleUrl(await compileRuntime(name));
const policy = await leaf("loader/policy.ts");
const shared = moduleUrl(
  await compileRuntime("loader/shared.ts", {
    "./snapshot.js": await leaf("loader/snapshot.ts"),
    "./python-snapshot.js": moduleUrl(
      'export function resolvePythonSnapshot() { throw Error("unexpected prepared snapshot read"); }',
    ),
  }),
);
const { preparePython } = await importRuntime("loader/python-prepare.ts", {
  "../gateway/token.js": await leaf("gateway/token.ts"),
  "./envelope.js": await leaf("loader/envelope.ts"),
  "./shared.js": shared,
  "./modules.js": moduleUrl(
    await compileRuntime("loader/modules.ts", {
      "./module-values.js": await leaf("loader/module-values.ts"),
      "./policy.js": policy,
      "./shared.js": shared,
    }),
  ),
  "./bindings.js": moduleUrl(
    await compileRuntime("loader/bindings.ts", {
      "../bindings/native-construction.js": await leaf(
        "bindings/native-construction.ts",
      ),
      "./policy.js": policy,
      "./shared.js": shared,
    }),
  ),
});
const instance = "019c0000000070008000000000000001";
const worker = "019c0000-0000-7000-8000-000000000002";
const version = "019c0000-0000-7000-8000-000000000003";
const key = `${instance}/${worker}/${version}`;
const token = "a".repeat(64);
const snapshot = {
  schemaVersion: 1,
  loaderKey: key,
  workerCodeSha256: "b".repeat(64),
  routeGeneration: 1,
  compatibilityDate: "2026-09-08",
  compatibilityFlags: ["python_workers", "python_dedicated_snapshot"],
  limits: { cpuMs: 1000, subRequests: 1000 },
  contentKind: "worker",
  mainModule: "main.py",
  modules: [
    {
      name: "main.py",
      type: "python",
      bytesBase64: Buffer.from("pass\n").toString("base64"),
    },
  ],
  moduleBindings: [],
  workerLoaders: [],
  browserBindings: [],
  env: { SECRET: "private-value" },
  bindings: [],
  services: [],
  scheduledTargets: [],
  cachePolicy: {
    enabled: false,
    crossVersionCache: false,
    failOpen: false,
    entrypoints: {},
  },
};
function request(headers = {}, method = "POST") {
  return new Request("http://loader/internal/prepare-python", {
    method,
    headers: {
      "x-open-compute-internal-token": token,
      "x-open-compute-loader-key": key,
      "x-open-compute-instance-id": instance,
      "x-open-compute-worker-code-sha256": "b".repeat(64),
      "x-open-compute-route-generation": "1",
      ...headers,
    },
  });
}
function host({
  prepare,
  revoke,
  value = snapshot,
  hostPolicyVersion = 1,
  sourceError,
} = {}) {
  const events = [];
  let captured;
  const env = {
    INTERNAL_TOKEN: token,
    OUTBOUND_NETWORK: {},
    DO_MAX_OBJECT_NAME_BYTES: "1024",
    DO_MAX_FETCH_BODY_BYTES: "1024",
    DO_DISPATCH_TIMEOUT_MS: "1000",
    DO_MAX_IN_FLIGHT_DISPATCHES: "10",
    RUNTIME_SOURCE: {
      async fetch(_url, init) {
        events.push(["source", JSON.parse(init.body).scope]);
        if (sourceError)
          return new Response(null, {
            status: 503,
            headers: { "x-open-compute-error-code": sourceError },
          });
        return Response.json(value);
      },
    },
    WORKER_LOADER_FACTORY: {
      hostPolicyVersion,
      getPrivate(namespace) {
        events.push(["grant", namespace]);
        return {
          async preparePython(code) {
            captured = code;
            return prepare ? prepare() : new Uint8Array(32).fill(42);
          },
        };
      },
      revoke(namespace) {
        events.push(["revoke", namespace]);
        revoke?.();
      },
    },
  };
  const ctx = {
    exports: {
      CacheTransport() {
        return {};
      },
    },
  };
  return { env, ctx, events, code: () => captured };
}

test("private prepare uses exact Python source and env, then revokes its single-use grant", async () => {
  const h = host();
  const response = await preparePython(request(), h.env, h.ctx);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.equal(response.headers.get("content-length"), "32");
  assert.deepEqual(
    new Uint8Array(await response.arrayBuffer()),
    new Uint8Array(32).fill(42),
  );
  assert.deepEqual(
    h.events.map(([operation]) => operation),
    ["source", "grant", "revoke"],
  );
  assert.equal(h.events[0][1], "preparation");
  assert.equal(h.events[1][1], h.events[2][1]);
  assert.equal(h.code().mainModule, "main.py");
  assert.deepEqual(h.code().modules, { "main.py": { py: "pass\n" } });
  assert.deepEqual(h.code().env, { SECRET: "private-value" });
  assert.equal(h.code().openComputeHostPolicy, true);
  assert.equal(
    h.code().openComputePrivateEnv.__OPEN_COMPUTE_PRIVATE_POLICY.validation,
    false,
  );
  assert.equal(h.code().limits.cpuMs, 1000);
});

test("private prepare rejects bad authentication, scope, descriptor and runtime before granting a Loader", async () => {
  for (const [headers, method, options, status] of [
    [{ "x-open-compute-internal-token": "wrong" }, "POST", {}, 404],
    [{}, "GET", {}, 422],
    [{ "x-open-compute-entrypoint": "Named" }, "POST", {}, 422],
    [{ "x-open-compute-loader-key": "wrong" }, "POST", {}, 422],
    [
      {},
      "POST",
      { value: { ...snapshot, workerCodeSha256: "c".repeat(64) } },
      422,
    ],
    [{}, "POST", { value: { ...snapshot, mainModule: "main.js" } }, 422],
    [{}, "POST", { hostPolicyVersion: 0 }, 503],
    [{}, "POST", { sourceError: "ARTIFACT_UNAVAILABLE" }, 503],
    [{}, "POST", { sourceError: "VERSION_NOT_READY" }, 409],
    [{}, "POST", { sourceError: "private-value" }, 422],
  ]) {
    const h = host(options);
    const response = await preparePython(
      request(headers, method),
      h.env,
      h.ctx,
    );
    assert.equal(response.status, status);
    assert.equal(await response.text(), "");
    assert.equal(
      h.events.some(([operation]) => operation === "grant"),
      false,
    );
  }
});

test("native errors, invalid snapshot size and revocation failures never return private values", async () => {
  for (const options of [
    {
      prepare() {
        throw Error("private-value import traceback");
      },
    },
    {
      prepare() {
        return new Uint8Array(15);
      },
    },
    {
      revoke() {
        throw Error("private-value grant registry failure");
      },
    },
  ]) {
    const h = host(options);
    const response = await preparePython(request(), h.env, h.ctx);
    assert.equal(response.status, 422);
    assert.equal(
      response.headers.get("x-open-compute-error-code"),
      "BUNDLE_RUNTIME_INVALID",
    );
    assert.equal(await response.text(), "");
    assert.equal(h.events.at(-1)[0], "revoke");
  }
});

test("overlapping preparations own distinct grants so one revocation cannot cancel the other", async () => {
  const h = host();
  const responses = await Promise.all([
    preparePython(request(), h.env, h.ctx),
    preparePython(request(), h.env, h.ctx),
  ]);
  assert.ok(responses.every((response) => response.status === 200));
  const granted = h.events
    .filter(([operation]) => operation === "grant")
    .map(([, namespace]) => namespace);
  const revoked = h.events
    .filter(([operation]) => operation === "revoke")
    .map(([, namespace]) => namespace);
  assert.equal(new Set(granted).size, 2);
  assert.deepEqual([...granted].sort(), [...revoked].sort());
});

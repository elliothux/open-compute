import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const { resolveSnapshot, snapshotWorkerCode, pythonPreparationCode } =
  await importRuntime("loader/shared.ts", {
    "./snapshot.js": moduleUrl("export function assertSnapshot() {}"),
    "./python-snapshot.js": moduleUrl(
      "export function resolvePythonSnapshot(env, snapshot, scope) { return env.snapshotRead(snapshot, scope); }",
    ),
  });

test("dynamic Worker validation delegates experimental flags to workerd", async () => {
  assert.deepEqual(
    await snapshotWorkerCode(
      {},
      {
        compatibilityDate: "2026-09-08",
        compatibilityFlags: ["experimental"],
        limits: { cpuMs: 30_000, subRequests: 1_000 },
      },
      "runtime",
    ),
    {
      compatibilityDate: "2026-09-08",
      compatibilityFlags: ["experimental"],
      allowExperimental: true,
      limits: { cpuMs: 30_000, subRequests: 1_000 },
    },
  );
});

test("runtime snapshot rejects a route generation changed during source resolution", async () => {
  let generation = 2;
  const requests = [];
  const env = {
    RUNTIME_SOURCE: {
      fetch: async (_url, init) => {
        requests.push(init);
        return Response.json({
          loaderKey:
            "019c0000000070008000000000000001/019c0000-0000-7000-8000-000000000002/019c0000-0000-7000-8000-000000000003",
          workerCodeSha256: "a".repeat(64),
          routeGeneration: generation,
        });
      },
    },
  };
  const envelope = {
    loaderKey:
      "019c0000000070008000000000000001/019c0000-0000-7000-8000-000000000002/019c0000-0000-7000-8000-000000000003",
    expected: "a".repeat(64),
    routeGeneration: 1,
  };
  await assert.rejects(
    resolveSnapshot(env, envelope, "runtime"),
    /VERSION_INVARIANT_VIOLATION/,
  );
  generation = 1;
  assert.equal(
    (await resolveSnapshot(env, envelope, "runtime")).routeGeneration,
    1,
  );
  assert.equal(
    requests[0].headers["x-open-compute-startup-generation"],
    undefined,
  );
  assert.equal(JSON.parse(requests[0].body).startupGeneration, undefined);
});

test("deployment startup rejects raw Python before any runtime preparation or download", async () => {
  await assert.rejects(
    () =>
      snapshotWorkerCode(
        {},
        {
          mainModule: "main.py",
          compatibilityDate: "2026-09-08",
          compatibilityFlags: ["python_workers"],
          limits: { cpuMs: 30_000, subRequests: 1_000 },
        },
        "runtime",
      ),
    /PYTHON_PREPARED_ARTIFACT_MISSING/,
  );
});

test("Python artifact is read only during cold assembly, never metadata resolution", async () => {
  const snapshot = {
    loaderKey: "immutable-key",
    workerCodeSha256: "b".repeat(64),
    contentKind: "worker",
    mainModule: "main.py",
    compatibilityDate: "2026-09-08",
    compatibilityFlags: ["python_workers"],
    limits: { cpuMs: 30_000, subRequests: 1_000 },
    pythonPreparedSha256: "a".repeat(64),
  };
  let reads = 0;
  const bytes = new Uint8Array(16);
  const env = {
    RUNTIME_SOURCE: { fetch: async () => Response.json(snapshot) },
    snapshotRead(value, scope) {
      assert.deepEqual(value, snapshot);
      assert.equal(scope, "runtime");
      reads += 1;
      return bytes;
    },
  };
  for (let index = 0; index < 2; index++)
    assert.deepEqual(
      await resolveSnapshot(
        env,
        { loaderKey: snapshot.loaderKey, expected: snapshot.workerCodeSha256 },
        "runtime",
      ),
      snapshot,
    );
  assert.equal(reads, 0);
  assert.equal(
    (await snapshotWorkerCode(env, snapshot, "runtime"))
      .openComputePythonSnapshot,
    bytes,
  );
  assert.equal(reads, 1);
  assert.throws(
    () => pythonPreparationCode(snapshot),
    /VERSION_INVARIANT_VIOLATION/,
  );
  await assert.rejects(
    snapshotWorkerCode(
      env,
      { ...snapshot, pythonPreparedSha256: "A".repeat(64) },
      "runtime",
    ),
    /VERSION_INVARIANT_VIOLATION/,
  );
  assert.equal(reads, 1);
  const { pythonPreparedSha256, ...raw } = snapshot;
  assert.ok(pythonPreparedSha256);
  assert.equal(pythonPreparationCode(raw).openComputePythonSnapshot, undefined);
});

test("WorkerCode forwards the Version limits object directly", async () => {
  const limits = { cpuMs: 30_000, subRequests: 10_000 };
  const code = await snapshotWorkerCode(
    {},
    {
      compatibilityDate: "2026-09-08",
      compatibilityFlags: [],
      limits,
    },
    "runtime",
  );
  assert.equal(code.limits, limits);
});

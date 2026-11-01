import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const shared = moduleUrl(
  await compileRuntime("loader/shared.ts", {
    "./snapshot.js": moduleUrl("export function assertSnapshot() {}"),
    "./python-snapshot.js": moduleUrl(
      'export function resolvePythonSnapshot() { throw Error("unused"); }',
    ),
  }),
);
const { resolvePythonSnapshot } = await importRuntime(
  "loader/python-snapshot.ts",
  {
    "./shared.js": shared,
    "../bindings/json-body.js": moduleUrl(
      await compileRuntime("bindings/json-body.ts"),
    ),
  },
);
const snapshot = {
  loaderKey: "immutable-key",
  workerCodeSha256: "a".repeat(64),
  pythonPreparedSha256: "b".repeat(64),
};
const binary = (bytes, headers = {}) =>
  new Response(bytes, {
    headers: { "content-type": "application/octet-stream", ...headers },
  });
const read = (response) =>
  resolvePythonSnapshot(
    { RUNTIME_SOURCE: { fetch: async () => response } },
    snapshot,
    "runtime",
  );
const stable = (code) => (error) => {
  assert.equal(error.stableCode, code);
  assert.equal(error.message, code);
  assert.equal(error.stack, `Error: ${code}`);
  return true;
};

test("prepared snapshot transfer authenticates its exact source and retained artifact", async () => {
  const bytes = new Uint8Array(32).fill(7);
  for (const scope of ["runtime", "validation", "probe", "preparation"]) {
    const value = await resolvePythonSnapshot(
      {
        RUNTIME_SOURCE: {
          async fetch(url, init) {
            assert.equal(
              url,
              "http://runtime-source/internal/runtime/v1/versions/python-snapshot",
            );
            assert.equal(init.method, "POST");
            assert.equal(
              init.headers["x-open-compute-internal-token"],
              undefined,
            );
            assert.equal(
              init.headers["x-open-compute-startup-generation"],
              undefined,
            );
            assert.deepEqual(JSON.parse(init.body), {
              key: snapshot.loaderKey,
              expectedWorkerCodeSha256: snapshot.workerCodeSha256,
              expectedPreparedSha256: snapshot.pythonPreparedSha256,
              scope,
            });
            return binary(bytes, { "content-length": "32" });
          },
        },
      },
      snapshot,
      scope,
    );
    assert.deepEqual(value, bytes);
  }
  assert.deepEqual(await read(binary(bytes)), bytes);
});

test("prepared snapshot refuses missing, truncated and invalidly framed binary bodies", async () => {
  for (const response of [
    binary(null),
    binary(new Uint8Array(15)),
    binary(new Uint8Array(16), { "content-length": "17" }),
    binary(new Uint8Array(16), { "content-length": "-1" }),
    binary(new Uint8Array(16), { "content-length": "x" }),
    binary(new Uint8Array(16), {
      "content-length": String(128 * 1024 * 1024 + 1),
    }),
    binary(new Uint8Array(16), { "content-encoding": "gzip" }),
    new Response("secret", { headers: { "content-type": "application/json" } }),
  ])
    await assert.rejects(read(response), stable("VERSION_INVARIANT_VIOLATION"));
});

test("private snapshot rejects stream overflow and hides transport or cleanup exceptions", async () => {
  let cancelled = false;
  let finishCancel;
  const cancellation = new Promise((resolve) => {
    finishCancel = resolve;
  });
  let count = 0;
  const chunk = new Uint8Array(1024 * 1024);
  const stream = new ReadableStream({
    pull(controller) {
      controller.enqueue(chunk);
      count += 1;
    },
    cancel() {
      cancelled = true;
      finishCancel();
    },
  });
  await assert.rejects(
    read(binary(stream)),
    stable("VERSION_INVARIANT_VIOLATION"),
  );
  await cancellation;
  assert.equal(cancelled, true);
  assert.ok(count <= 131);
  const broken = new ReadableStream({
    start(controller) {
      controller.error(Error("private path and secret"));
    },
  });
  await assert.rejects(
    read(binary(broken)),
    stable("VERSION_INVARIANT_VIOLATION"),
  );
  const invalid = new ReadableStream({
    cancel() {
      cancelled = true;
      throw Error("private cleanup secret");
    },
  });
  await assert.rejects(
    read(
      new Response(invalid, {
        status: 503,
        headers: { "x-open-compute-error-code": "ARTIFACT_UNAVAILABLE" },
      }),
    ),
    stable("ARTIFACT_UNAVAILABLE"),
  );
  assert.equal(cancelled, true);
  await assert.rejects(
    resolvePythonSnapshot(
      {
        RUNTIME_SOURCE: {
          fetch: async () => {
            throw Error("private network secret");
          },
        },
      },
      snapshot,
      "runtime",
    ),
    stable("RUNTIME_UNAVAILABLE"),
  );
  for (const code of [null, "private secret", "A".repeat(65)]) {
    const headers = code === null ? {} : { "x-open-compute-error-code": code };
    await assert.rejects(
      read(new Response(null, { status: 503, headers })),
      stable("VERSION_INVARIANT_VIOLATION"),
    );
  }
});

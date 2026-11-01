import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { loadPortableFixtures } from "./adapters/fixtures.ts";
import { canonicalJson, observe } from "./adapters/observations.ts";
import { cloudflareProject, openComputeProject } from "./adapters/projects.ts";

async function fixture(overrides = {}) {
  const root = await mkdtemp(join(tmpdir(), "open-compute-portable-fixture-"));
  const directory = join(root, "workers", "portable");
  await mkdir(join(directory, "src"), { recursive: true });
  await writeFile(
    join(directory, "src/index.ts"),
    "export default { fetch() { return Response.json({ ok: true }); } };\n",
  );
  await writeFile(
    join(directory, "contract.json"),
    `${JSON.stringify(
      {
        schemaVersion: 1,
        id: "workers/portable/example",
        contracts: ["workers.runtime.common"],
        source: "src/index.ts",
        bindings: {},
        observations: [
          {
            method: "POST",
            path: "/",
            body: { json: { b: 2, a: 1 } },
            expect: { status: 200, json: { ok: true } },
          },
        ],
        normalization: [],
        cleanup: { cloudflare: ["worker"], openCompute: ["worker"] },
        ...overrides,
      },
      null,
      2,
    )}\n`,
  );
  return root;
}

test("portable fixture schema is strict and its digest covers the whole fixture", async () => {
  const root = await fixture();
  const [loaded] = await loadPortableFixtures(root);
  assert.equal(loaded.id, "workers/portable/example");
  assert.equal(loaded.observations[0].body instanceof Uint8Array, true);
  const original = loaded.sourceSha256;
  const contract = join(loaded.root, "contract.json");
  await writeFile(contract, `${await readFile(contract, "utf8")}\n`);
  const [changed] = await loadPortableFixtures(root);
  assert.notEqual(changed.sourceSha256, original);

  const bound = await fixture({
    bindings: { KV: { type: "kv_namespace" } },
    cleanup: {
      cloudflare: ["worker", "kv_namespace"],
      openCompute: ["worker", "kv_namespace"],
    },
  });
  const [loadedBound] = await loadPortableFixtures(bound);
  assert.deepEqual(loadedBound.bindings, { KV: { type: "kv_namespace" } });
  const r2Bound = await fixture({
    bindings: { BUCKET: { type: "r2_bucket" } },
    cleanup: {
      cloudflare: ["worker", "r2_bucket"],
      openCompute: ["worker", "r2_bucket"],
    },
  });
  const [loadedR2Bound] = await loadPortableFixtures(r2Bound);
  assert.deepEqual(loadedR2Bound.bindings, { BUCKET: { type: "r2_bucket" } });
  const productBound = await fixture({
    bindings: {
      OBJECTS: { type: "do_namespace", className: "PortableObject" },
      EVENTS: { type: "queue_producer" },
      FLOW: { type: "workflow", className: "PortableWorkflow" },
    },
    cleanup: {
      cloudflare: ["worker", "do_namespace", "queue_producer", "workflow"],
      openCompute: ["worker", "do_namespace", "queue_producer", "workflow"],
    },
  });
  const [loadedProductBound] = await loadPortableFixtures(productBound);
  assert.deepEqual(loadedProductBound.bindings, {
    OBJECTS: { type: "do_namespace", className: "PortableObject" },
    EVENTS: { type: "queue_producer" },
    FLOW: { type: "workflow", className: "PortableWorkflow" },
  });
  const ocProduct = openComputeProject(
    loadedProductBound,
    "portable-product",
    "0123456789abcdef0123456789abcdef",
    {},
    { EVENTS: "portable-queue", FLOW: "portable-workflow" },
  );
  assert.equal(ocProduct.worker.workersDev, false);
  assert.deepEqual(ocProduct.worker.env, {
    OBJECTS: {
      type: "durable-object",
      worker: "portable-product",
      exportName: "PortableObject",
    },
    EVENTS: { type: "queue", name: "portable-queue" },
    FLOW: {
      type: "workflow",
      worker: "portable-product",
      exportName: "PortableWorkflow",
      name: "portable-workflow",
    },
  });
  assert.deepEqual(ocProduct.worker.exports, {
    PortableObject: { type: "durable-object", storage: "sqlite" },
    PortableWorkflow: { type: "workflow", name: "portable-workflow" },
  });
  const cfProduct = cloudflareProject(
    loadedProductBound,
    "portable-product",
    "0123456789abcdef0123456789abcdef",
    {},
    { EVENTS: "portable-queue", FLOW: "portable-workflow" },
  );
  assert.deepEqual(cfProduct.worker.env, ocProduct.worker.env);
  assert.deepEqual(cfProduct.worker.exports, ocProduct.worker.exports);
  assert.equal(cfProduct.worker.workersDev, true);
  const missingClass = await fixture({
    bindings: { OBJECTS: { type: "do_namespace" } },
    cleanup: {
      cloudflare: ["worker", "do_namespace"],
      openCompute: ["worker", "do_namespace"],
    },
  });
  await assert.rejects(loadPortableFixtures(missingClass), /className/);
  const unsupported = await fixture({
    bindings: { KV: { type: "unsupported" } },
  });
  await assert.rejects(
    loadPortableFixtures(unsupported),
    /binding type is unsupported/,
  );
  const unknown = await fixture({ unexpected: true });
  await assert.rejects(loadPortableFixtures(unknown), /unsupported fields/);
});

test("canonical JSON comparison recursively sorts object keys without reordering arrays", () => {
  assert.deepEqual(canonicalJson({ z: [{ b: 2, a: 1 }], a: { d: 4, c: 3 } }), {
    a: { c: 3, d: 4 },
    z: [{ a: 1, b: 2 }],
  });
});

test("Worker Loader uses a native binding without provisioned resource identity or cleanup", async () => {
  const root = await fixture({
    bindings: { LOADER: { type: "worker_loader" } },
  });
  const [loaded] = await loadPortableFixtures(root);
  for (const project of [openComputeProject, cloudflareProject]) {
    const config = project(
      loaded,
      "dynamic-parent",
      "0123456789abcdef0123456789abcdef",
    );
    assert.deepEqual(config.worker.env.LOADER, { type: "worker-loader" });
    assert.throws(
      () =>
        project(loaded, "dynamic-parent", "0123456789abcdef0123456789abcdef", {
          LOADER: "invented-resource",
        }),
      /binding/,
    );
  }
  const invalid = await fixture({
    bindings: { LOADER: { type: "worker_loader", id: "invented" } },
  });
  await assert.rejects(loadPortableFixtures(invalid), /unsupported fields/);
});

test("open-compute observations preserve the explicit route Host header", async () => {
  const server = createServer((request, response) => {
    assert.equal(request.headers.host, "portable-route.invalid");
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ ok: true }));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    assert.notEqual(address, null);
    assert.equal(typeof address, "object");
    const result = await observe(
      `http://127.0.0.1:${address.port}/`,
      {
        id: "workers/portable/host",
        observations: [
          {
            method: "GET",
            path: "/",
            headers: {},
            expect: { status: 200, json: { ok: true } },
          },
        ],
      },
      "open-compute",
      { host: "portable-route.invalid", connection: "close" },
    );
    assert.deepEqual(result, [{ status: 200, json: { ok: true } }]);
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("observation mismatches identify the first canonical JSON path", async () => {
  const server = createServer((_request, response) => {
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ outer: [{ value: "actual" }] }));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    assert.notEqual(address, null);
    assert.equal(typeof address, "object");
    await assert.rejects(
      observe(
        `http://127.0.0.1:${address.port}/`,
        {
          id: "workers/portable/difference",
          observations: [
            {
              method: "GET",
              path: "/",
              headers: {},
              expect: { status: 200, json: { outer: [{ value: "expected" }] } },
            },
          ],
        },
        "open-compute",
      ),
      /\$\.outer\[0\]\.value: actual="actual"; expected="expected"/,
    );
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("Cloudflare refresh collects normalized responses, preserves unchanged recordings, and records provenance once", async () => {
  const { refreshRecordings } = await import("./differential/runner.ts");
  const root = await fixture();
  await mkdir(join(root, "test/conformance"), { recursive: true });
  const baselinePath = join(root, "test/conformance/baseline.json");
  await writeFile(baselinePath, '{"schemaVersion":1}\n');
  const [loaded] = await loadPortableFixtures(root);
  const unchangedRoot = await fixture({ id: "workers/portable/unchanged" });
  const [unchanged] = await loadPortableFixtures(unchangedRoot);
  const unchangedPath = join(unchanged.root, "contract.json");
  const unchangedBytes = await readFile(unchangedPath, "utf8");
  const unchangedTime = (await stat(unchangedPath, { bigint: true })).mtimeNs;
  const changedPath = join(loaded.root, "contract.json");
  const server = createServer((_request, response) => {
    response.writeHead(201, { "content-type": "application/json" });
    response.end('{"z":2,"a":1}');
  });
  await new Promise((accept) => server.listen(0, "127.0.0.1", accept));
  try {
    const url = `http://127.0.0.1:${server.address().port}/`;
    await assert.rejects(
      observe(url, loaded, "cloudflare"),
      /observation differs/,
    );
    const cloudflare = await observe(url, loaded, "cloudflare", {}, false);
    assert.deepEqual(cloudflare, [{ status: 201, json: { a: 1, z: 2 } }]);
    await assert.rejects(
      observe(url, loaded, "open-compute"),
      /observation differs/,
    );
    const current = {
      ...loaded,
      observations: loaded.observations.map((entry) => ({
        ...entry,
        expect: cloudflare[0],
      })),
    };
    assert.deepEqual(await observe(url, current, "open-compute"), cloudflare);
    const results = [
      { id: loaded.id, status: "passed", cloudflare },
      {
        id: unchanged.id,
        status: "passed",
        cloudflare: unchanged.observations.map((entry) => entry.expect),
      },
    ];
    const original = await readFile(changedPath, "utf8");
    await assert.rejects(
      refreshRecordings(
        root,
        [loaded, unchanged],
        [results[0]],
        "qualification",
      ),
      /complete, passing/,
    );
    await assert.rejects(
      refreshRecordings(
        root,
        [loaded],
        [{ ...results[0], status: "failed" }],
        "qualification",
      ),
      /complete, passing/,
    );
    await assert.rejects(
      refreshRecordings(root, [loaded], [results[0]], "a".repeat(32)),
      /alias/,
    );
    await assert.rejects(
      refreshRecordings(root, [loaded], [results[0]], "qualification", [
        '"z": 2',
      ]),
      /private identity or secret/,
    );
    assert.equal(await readFile(changedPath, "utf8"), original);
    await refreshRecordings(
      root,
      [loaded, unchanged],
      results,
      "qualification",
    );
    assert.deepEqual(
      JSON.parse(await readFile(changedPath, "utf8")).observations[0].expect,
      cloudflare[0],
    );
    assert.equal(await readFile(unchangedPath, "utf8"), unchangedBytes);
    assert.equal(
      (await stat(unchangedPath, { bigint: true })).mtimeNs,
      unchangedTime,
    );
    const baseline = JSON.parse(await readFile(baselinePath, "utf8"));
    assert.deepEqual(baseline.portableRecording, {
      date: new Date().toISOString().slice(0, 10),
      cfVersion: "1.0.0-beta.12",
      accountAlias: "qualification",
    });
    const reloaded = (await loadPortableFixtures(root))[0];
    await refreshRecordings(root, [reloaded], [results[0]], "qualification");
    assert.equal(
      await readFile(baselinePath, "utf8"),
      `${JSON.stringify(baseline, null, 2)}\n`,
    );
  } finally {
    await new Promise((accept, reject) =>
      server.close((error) => (error ? reject(error) : accept())),
    );
  }
});

test("recorded mode rejects external endpoints and live refresh before any mutation", async () => {
  const { validatedEndpoint } = await import("./differential/run-context.ts");
  const { runDifferential } = await import("./differential/runner.ts");
  for (const url of [
    "https://example.com",
    "http://example.com",
    "http://secret@127.0.0.1",
    "http://127.0.0.1/path",
  ]) {
    process.env.OPEN_COMPUTE_RECORDED_TEST_ENDPOINT = url;
    assert.throws(
      () => validatedEndpoint("OPEN_COMPUTE_RECORDED_TEST_ENDPOINT", true),
      /endpoint/,
    );
  }
  process.env.OPEN_COMPUTE_RECORDED_TEST_ENDPOINT = "http://127.0.0.1:8787";
  assert.equal(
    validatedEndpoint("OPEN_COMPUTE_RECORDED_TEST_ENDPOINT", true).hostname,
    "127.0.0.1",
  );
  delete process.env.OPEN_COMPUTE_RECORDED_TEST_ENDPOINT;
  await assert.rejects(
    runDifferential("/unused", [], true, true),
    /cannot refresh Cloudflare/,
  );
});

import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { canonicalJson, cloudflareProject, loadPortableFixtures, observe, openComputeProject } from "./adapters.ts";

async function fixture(overrides = {}) {
  const root = await mkdtemp(join(tmpdir(), "open-compute-portable-fixture-"));
  const directory = join(root, "workers", "portable");
  await mkdir(join(directory, "src"), { recursive: true });
  await writeFile(join(directory, "src/index.ts"), "export default { fetch() { return Response.json({ ok: true }); } };\n");
  await writeFile(join(directory, "contract.json"), `${JSON.stringify({
    schemaVersion: 1,
    id: "workers/portable/example",
    contracts: ["workers.runtime.common"],
    source: "src/index.ts",
    bindings: {},
    observations: [{ method: "POST", path: "/", body: { json: { b: 2, a: 1 } }, expect: { status: 200, json: { ok: true } } }],
    normalization: [],
    cleanup: { cloudflare: ["worker"], openCompute: ["worker"] },
    ...overrides,
  }, null, 2)}\n`);
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
    cleanup: { cloudflare: ["worker", "kv_namespace"], openCompute: ["worker", "kv_namespace"] },
  });
  const [loadedBound] = await loadPortableFixtures(bound);
  assert.deepEqual(loadedBound.bindings, { KV: { type: "kv_namespace" } });
  const r2Bound = await fixture({
    bindings: { BUCKET: { type: "r2_bucket" } },
    cleanup: { cloudflare: ["worker", "r2_bucket"], openCompute: ["worker", "r2_bucket"] },
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
  assert.equal(ocProduct.workers_dev, false);
  assert.deepEqual(ocProduct.durable_objects, {
    bindings: [{ name: "OBJECTS", class_name: "PortableObject" }],
  });
  assert.deepEqual(ocProduct.migrations, [{ tag: "v1", new_sqlite_classes: ["PortableObject"] }]);
  assert.deepEqual(ocProduct.queues, {
    producers: [{ binding: "EVENTS", queue: "portable-queue" }],
  });
  assert.deepEqual(ocProduct.workflows, [{
    binding: "FLOW", name: "portable-workflow", class_name: "PortableWorkflow",
  }]);
  const cfProduct = cloudflareProject(
    loadedProductBound,
    "portable-product",
    "0123456789abcdef0123456789abcdef",
    {},
    { EVENTS: "portable-queue", FLOW: "portable-workflow" },
  );
  assert.deepEqual(cfProduct.durable_objects, {
    bindings: [{ name: "OBJECTS", class_name: "PortableObject" }],
  });
  assert.deepEqual(cfProduct.migrations, [{ tag: "v1", new_sqlite_classes: ["PortableObject"] }]);
  assert.deepEqual(cfProduct.queues, {
    producers: [{ binding: "EVENTS", queue: "portable-queue" }],
  });
  assert.deepEqual(cfProduct.workflows, [{
    binding: "FLOW", name: "portable-workflow", class_name: "PortableWorkflow",
  }]);
  assert.equal(cfProduct.workers_dev, true);
  const missingClass = await fixture({
    bindings: { OBJECTS: { type: "do_namespace" } },
    cleanup: { cloudflare: ["worker", "do_namespace"], openCompute: ["worker", "do_namespace"] },
  });
  await assert.rejects(loadPortableFixtures(missingClass), /className/);
  const unsupported = await fixture({ bindings: { KV: { type: "unsupported" } } });
  await assert.rejects(loadPortableFixtures(unsupported), /binding type is unsupported/);
  const unknown = await fixture({ unexpected: true });
  await assert.rejects(loadPortableFixtures(unknown), /unsupported fields/);
});

test("canonical JSON comparison recursively sorts object keys without reordering arrays", () => {
  assert.deepEqual(canonicalJson({ z: [{ b: 2, a: 1 }], a: { d: 4, c: 3 } }), {
    a: { c: 3, d: 4 },
    z: [{ a: 1, b: 2 }],
  });
});

test("open-compute observations preserve the explicit route Host header", async () => {
  const server = createServer((request, response) => {
    assert.equal(request.headers.host, "portable-route.invalid");
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ ok: true }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    assert.notEqual(address, null);
    assert.equal(typeof address, "object");
    const result = await observe(`http://127.0.0.1:${address.port}/`, {
      id: "workers/portable/host",
      observations: [{ method: "GET", path: "/", headers: {}, expect: { status: 200, json: { ok: true } } }],
    }, "open-compute", { host: "portable-route.invalid", connection: "close" });
    assert.deepEqual(result, [{ status: 200, json: { ok: true } }]);
  } finally {
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

test("observation mismatches identify the first canonical JSON path", async () => {
  const server = createServer((_request, response) => {
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ outer: [{ value: "actual" }] }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    assert.notEqual(address, null);
    assert.equal(typeof address, "object");
    await assert.rejects(observe(`http://127.0.0.1:${address.port}/`, {
      id: "workers/portable/difference",
      observations: [{ method: "GET", path: "/", headers: {}, expect: { status: 200, json: { outer: [{ value: "expected" }] } } }],
    }, "open-compute"), /\$\.outer\[0\]\.value: actual="actual"; expected="expected"/);
  } finally {
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

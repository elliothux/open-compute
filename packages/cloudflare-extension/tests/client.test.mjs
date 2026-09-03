import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import Cloudflare, { APIError } from "cloudflare";
import {
  createOpenComputeExtension,
  OPEN_COMPUTE_EXTENSION_OPERATIONS,
  OPEN_COMPUTE_EXTENSION_SCHEMA_SHA256,
} from "../src/index.ts";

test("derives its contract digest and operation catalog from the root OpenAPI authority", async () => {
  const bytes = await readFile(new URL("../../../openapi/open-compute-extension.json", import.meta.url));
  const contract = JSON.parse(bytes.toString("utf8"));
  assert.equal(createHash("sha256").update(bytes).digest("hex"), OPEN_COMPUTE_EXTENSION_SCHEMA_SHA256);
  const declared = Object.entries(contract.paths).flatMap(([path, methods]) =>
    Object.entries(methods).map(([method, operation]) => ({
      method: method.toUpperCase(),
      path,
      operationId: operation.operationId,
    })),
  ).sort((left, right) => left.operationId.localeCompare(right.operationId));
  assert.deepEqual(OPEN_COMPUTE_EXTENSION_OPERATIONS, declared);
  assert.equal(declared.length, 18);
  for (const methods of Object.values(contract.paths)) {
    for (const [method, operation] of Object.entries(methods)) {
      if (method === "post" && operation["x-open-compute-request-body"] === "none") {
        assert.equal(operation.requestBody, undefined);
      } else if (method === "post") {
        assert.equal(operation["x-open-compute-request-body"], "json");
        assert.equal(operation.requestBody.required, true);
      }
    }
  }
});

test("reuses the official Cloudflare transport for extension operations", async () => {
  const requests = [];
  const client = new Cloudflare({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 0,
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return new Response(JSON.stringify({
        success: true,
        errors: [],
        messages: [],
        result: { release: "dev", wrangler_version: "4.127.1", compatibility_date: { minimum: "2026-08-30", maximum: "2026-08-30" }, compatibility_flags: [], endpoints: {}, deviations: [] },
      }), { headers: { "content-type": "application/json" } });
    },
  });
  const result = await createOpenComputeExtension(client).capabilities.get();
  assert.equal(result.release, "dev");
  assert.equal(requests.length, 1);
  assert.equal(requests[0].init.method, "GET");
  assert.equal(requests[0].url, "https://compute.example/client/v4/open-compute/capabilities");
  assert.equal(new Headers(requests[0].init.headers).get("authorization"), "Bearer test-token");
});

test("keeps bodyless extension POSTs bodyless through the official transport", async () => {
  const requests = [];
  const client = new Cloudflare({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 0,
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return new Response(JSON.stringify({ success: true, errors: [], messages: [], result: { state: "paused", pending: 0, running: 0 } }), { headers: { "content-type": "application/json" } });
    },
  });
  await createOpenComputeExtension(client).scheduler.pause();
  assert.equal(requests[0].init.method, "POST");
  assert.equal(requests[0].init.body, undefined);
  assert.equal(new Headers(requests[0].init.headers).get("content-type"), null);
});

test("sends typed restore JSON and returns the restored resource identity", async () => {
  const requests = [];
  const client = new Cloudflare({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 0,
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return new Response(JSON.stringify({
        success: true,
        errors: [],
        messages: [],
        result: { id: "restored-id", name: "restored-db", kind: "d1_database", created_on: "2026-09-03T00:00:00Z" },
      }), { headers: { "content-type": "application/json" } });
    },
  });

  const restored = await createOpenComputeExtension(client).backups.d1.restore(
    "account",
    "backup",
    { name: "restored-db" },
  );
  assert.equal(restored.id, "restored-id");
  assert.equal(restored.kind, "d1_database");
  assert.equal(requests.length, 1);
  assert.equal(requests[0].init.method, "POST");
  assert.equal(new Headers(requests[0].init.headers).get("content-type"), "application/json");
  assert.equal(requests[0].init.body, JSON.stringify({ name: "restored-db" }));
});

test("preserves official APIError and retry behavior", async () => {
  let attempts = 0;
  const retrying = new Cloudflare({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 1,
    fetch: async () => {
      attempts += 1;
      if (attempts === 1) return new Response("temporary", { status: 503 });
      return new Response(JSON.stringify({ success: true, errors: [], messages: [], result: { state: "ready", version: "dev", components: [] } }), { headers: { "content-type": "application/json" } });
    },
  });
  assert.equal((await createOpenComputeExtension(retrying).system.status()).state, "ready");
  assert.equal(attempts, 2);

  const failing = retrying.withOptions({ maxRetries: 0, fetch: async () => new Response(JSON.stringify({ success: false, result: null, errors: [{ code: 9100000, message: "denied" }], messages: [] }), { status: 403, headers: { "content-type": "application/json" } }) });
  await assert.rejects(createOpenComputeExtension(failing).capabilities.get(), APIError);
});

test("rejects path segments that URL normalization could consume", () => {
  const extension = createOpenComputeExtension(new Cloudflare({ apiToken: "test", maxRetries: 0 }));
  for (const value of ["", ".", ".."]) {
    assert.throws(() => extension.workers.endpoints(value, "worker"), /invalid extension path segment/);
    assert.throws(() => extension.workers.endpoints("account", value), /invalid extension path segment/);
  }
});

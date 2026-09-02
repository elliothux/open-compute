import assert from "node:assert/strict";
import test from "node:test";
import { z } from "zod";
import {
  OperatorApiError,
  OperatorProtocolError,
  createOperatorClient,
  parseAccountId,
  parseResourceId,
  readBoundedStreamBytes,
} from "../dist/index.js";
import { OperatorTransport } from "../dist/transport.js";

const baseUrl = new URL("http://127.0.0.1:8788/operator/api/v1/");

test("OperatorTransport rejects invalid baseUrl roots", () => {
  assert.throws(
    () => new OperatorTransport({
      baseUrl: new URL("http://127.0.0.1:8788/v1/"),
      getAccessToken: () => "token",
    }),
    OperatorProtocolError,
  );
  assert.throws(
    () => new OperatorTransport({
      baseUrl: new URL("http://127.0.0.1:8788/operator/api/v1"),
      getAccessToken: () => "token",
    }),
    OperatorProtocolError,
  );
});

test("OperatorTransport requires a token before sending requests", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => null,
  });
  await assert.rejects(
    transport.requestJson("GET", "account", z.object({ accountId: z.string() })),
    error => {
      assert.ok(error instanceof OperatorApiError);
      assert.equal(error.status, 401);
      assert.equal(error.code, "admin_auth_required");
      return true;
    },
  );
});

test("OperatorTransport validates success JSON with the supplied schema", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(JSON.stringify({
      accountId: "01900000-0000-7000-8000-000000000001",
    }), { status: 200, headers: { "content-type": "application/json" } }),
  });
  const body = await transport.requestJson(
    "GET",
    "account",
    z.strictObject({ accountId: z.string() }),
  );
  assert.equal(body.accountId, "01900000-0000-7000-8000-000000000001");
});

test("OperatorTransport rejects malformed success payloads", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(JSON.stringify({ unexpected: true }), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  });
  await assert.rejects(
    transport.requestJson("GET", "account", z.strictObject({ accountId: z.string() })),
    OperatorProtocolError,
  );
});

test("OperatorTransport maps stable API errors", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(JSON.stringify({
      ok: false,
      error: {
        code: "route_not_found",
        message: "route was not found",
        requestId: "req-1",
      },
    }), { status: 404, headers: { "content-type": "application/json" } }),
  });
  await assert.rejects(
    transport.requestJson("GET", "missing", z.object({ ok: z.literal(true) })),
    error => {
      assert.ok(error instanceof OperatorApiError);
      assert.equal(error.status, 404);
      assert.equal(error.code, "route_not_found");
      assert.equal(error.requestId, "req-1");
      return true;
    },
  );
});

test("OperatorTransport preserves deployment referrer conflicts", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(JSON.stringify({
      ok: false,
      error: {
        code: "DEPLOYMENT_REFERENCED",
        message: "deployment still has registered referrers",
        requestId: "req-deployment",
      },
    }), { status: 409, headers: { "content-type": "application/json" } }),
  });
  await assert.rejects(
    transport.requestJson("DELETE", "deployment", z.object({ ok: z.literal(true) })),
    error => {
      assert.ok(error instanceof OperatorApiError);
      assert.equal(error.code, "deployment_referenced");
      assert.equal(error.requestId, "req-deployment");
      return true;
    },
  );
});

test("createOperatorClient exposes resource namespaces", () => {
  const client = createOperatorClient({
    baseUrl,
    getAccessToken: () => "admin-secret",
  });
  assert.equal(typeof client.system.status, "function");
  assert.equal(typeof client.workers.list, "function");
  assert.equal(typeof client.workers.createDeployment, "function");
  assert.equal(typeof client.kv.listNamespaces, "function");
  assert.equal(typeof client.platform.scheduler, "function");
});

test("OperatorTransport forwards abort signals and idempotency headers", async () => {
  const controller = new AbortController();
  controller.abort();
  let capturedInit;
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async (_url, init) => {
      capturedInit = init;
      throw new DOMException("Aborted", "AbortError");
    },
  });
  await assert.rejects(
    transport.requestJson("POST", "accounts/a/workers", z.object({ workerId: z.string() }), {
      idempotencyKey: "create-worker-1",
      signal: controller.signal,
      body: { name: "demo" },
    }),
    error => error instanceof DOMException && error.name === "AbortError",
  );
  assert.equal(capturedInit?.signal, controller.signal);
  assert.equal(capturedInit?.headers?.get("idempotency-key"), "create-worker-1");
});

test("OperatorTransport forwards custom headers on binary uploads", async () => {
  let capturedInit;
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async (_url, init) => {
      capturedInit = init;
      return new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array([1, 2, 3]));
          controller.close();
        },
      }), { status: 200, headers: { "content-length": "3" } });
    },
  });
  const download = await transport.requestBinary("PUT", "accounts/a/workers/b/deployment-uploads/c/objects/d", {
    body: new Uint8Array([1, 2, 3]),
    contentType: "application/octet-stream",
    headers: { "content-length": "3" },
  });
  const bytes = await readBoundedStreamBytes(download.body, 1024);
  assert.deepEqual(bytes, new Uint8Array([1, 2, 3]));
  assert.equal(capturedInit?.headers?.get("authorization"), "Bearer admin-secret");
  assert.equal(capturedInit?.headers?.get("content-type"), "application/octet-stream");
  assert.equal(capturedInit?.headers?.get("content-length"), "3");
});

test("OperatorTransport streams bounded binary downloads", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array([1, 2]));
        controller.enqueue(new Uint8Array([3]));
        controller.close();
      },
    }), { status: 200, headers: { "content-length": "3" } }),
  });
  const download = await transport.requestBinary("GET", "accounts/a/r2/buckets/b/objects/demo", {
    maxBytes: 8,
  });
  assert.equal(download.contentLength, 3);
  const bytes = await readBoundedStreamBytes(download.body, 8);
  assert.deepEqual(bytes, new Uint8Array([1, 2, 3]));
});

test("OperatorTransport rejects binary downloads above the declared content length", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array([1]));
        controller.close();
      },
    }), { status: 200, headers: { "content-length": "9" } }),
  });
  await assert.rejects(
    transport.requestBinary("GET", "accounts/a/r2/buckets/b/objects/demo", { maxBytes: 8 }),
    OperatorProtocolError,
  );
});

test("OperatorTransport parses bounded binary error JSON", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response(JSON.stringify({
      ok: false,
      error: {
        code: "resource_not_found",
        message: "object was not found",
        requestId: "req-r2",
      },
    }), {
      status: 404,
      headers: { "content-type": "application/json", "content-length": "120" },
    }),
  });
  await assert.rejects(
    transport.requestBinary("GET", "accounts/a/r2/buckets/b/objects/demo"),
    error => {
      assert.ok(error instanceof OperatorApiError);
      assert.equal(error.status, 404);
      assert.equal(error.code, "resource_not_found");
      assert.equal(error.requestId, "req-r2");
      return true;
    },
  );
});

test("createOperatorClient rejects invalid KV namespace params before fetch", async () => {
  const client = createOperatorClient({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => {
      throw new Error("fetch should not run for invalid params");
    },
  });
  await assert.rejects(
    async () => client.kv.createNamespace({
      accountId: parseAccountId("01900000-0000-7000-8000-000000000001"),
      name: "",
      idempotencyKey: "create-kv",
    }),
    error => error instanceof OperatorProtocolError,
  );
});

test("createOperatorClient accepts AbortSignal on strict catalog list params", async () => {
  let fetchCalled = false;
  const client = createOperatorClient({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => {
      fetchCalled = true;
      return new Response(JSON.stringify({ workers: [], listComplete: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    },
  });
  const accountId = parseAccountId("01900000-0000-7000-8000-000000000001");
  const body = await client.workers.list({
    accountId,
    limit: 100,
    signal: new AbortController().signal,
  });
  assert.equal(fetchCalled, true);
  assert.deepEqual(body.workers, []);
});

test("createOperatorClient accepts Durable Object force-delete params", async () => {
  let capturedUrl;
  let capturedInit;
  const client = createOperatorClient({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async (url, init) => {
      capturedUrl = String(url);
      capturedInit = init;
      return new Response(null, { status: 204 });
    },
  });
  await client.durableObjects.deleteNamespace({
    accountId: parseAccountId("01900000-0000-7000-8000-000000000001"),
    namespaceId: parseResourceId("01900000-0000-7000-8000-000000000002"),
    idempotencyKey: "delete-do-namespace",
    force: true,
  });
  assert.equal(capturedUrl?.endsWith("?force=true"), true);
  assert.equal(capturedInit?.method, "DELETE");
  assert.equal(capturedInit?.headers?.get("idempotency-key"), "delete-do-namespace");
});

test("OperatorTransport rejects oversize JSON responses", async () => {
  const transport = new OperatorTransport({
    baseUrl,
    getAccessToken: () => "admin-secret",
    fetch: async () => new Response("x".repeat(4 * 1024 * 1024 + 1), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  });
  await assert.rejects(
    transport.requestJson("GET", "account", z.object({ accountId: z.string() })),
    OperatorProtocolError,
  );
});

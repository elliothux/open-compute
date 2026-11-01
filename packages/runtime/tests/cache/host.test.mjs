import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const shared = moduleUrl(`

  export const INTERNAL_HEADERS = ["x-internal-fixture"];
  export const currentStartupGeneration = () => "generation";
  export function bindingError(code) {
    const error = Object.assign(new Error(code), {stableCode:code});
    error.stack = "Error: " + code;
    return error;
  }
`);
const cloudflare = moduleUrl(
  `export class WorkerEntrypoint {constructor(ctx,env){this.ctx=ctx;this.env=env;}}`,
);
const privateTransport = moduleUrl(
  await compileRuntime("bindings/private-transport.ts", {
    "../loader/shared.js": shared,
  }),
);
const adapter = moduleUrl(
  await compileRuntime("cache/native-adapter.ts", {
    "../loader/shared.js": shared,
  }),
);
const { CacheTransport, CacheWriteTransport } = await importRuntime(
  "cache/host.ts",
  {
    "cloudflare:workers": cloudflare,
    "../loader/shared.js": shared,
    "../bindings/private-transport.js": privateTransport,
    "./native-adapter.js": adapter,
  },
);
const props = {
  instanceId: "instance",
  workerId: "worker",
  versionId: "version",
  entrypoint: "default",
  descriptorSha256: "ab".repeat(32),
  automaticEnabled: true,
  crossVersionCache: false,
};
const host = (backend) =>
  new CacheTransport({ props }, { BINDING_BACKEND: backend });
const contextHeader = "x-open-compute-cache-write-context";
const payload = (context) =>
  new Request("https://cache.test/key", {
    method: "PUT",
    duplex: "half",
    headers: { [contextHeader]: JSON.stringify(context) },
    body: "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 3\r\n\r\nabc",
  });

test("Cache match preserves opaque encoding and strips every private response header", async () => {
  const transport = host({
    async fetch(url, init) {
      assert.ok(url.endsWith("/match"));
      assert.equal(init.encodeResponseBody, "manual");
      assert.equal(init.headers["x-open-compute-binding-token"], undefined);
      assert.equal(
        init.headers["x-open-compute-startup-generation"],
        undefined,
      );
      assert.equal(init.headers["x-open-compute-worker-id"], props.workerId);
      assert.equal(
        init.headers["x-open-compute-descriptor-sha256"],
        props.descriptorSha256,
      );
      return new Response("encoded", {
        headers: {
          "content-encoding": "gzip",
          "x-open-compute-cache-status": "HIT",
          "x-open-compute-cache-hit": "1",
          "x-open-compute-cache-fence": "2",
          "x-open-compute-private-secret": "secret",
          "cf-cache-status": "HIT",
        },
      });
    },
  });
  const lookup = await transport.match(
    "automatic",
    undefined,
    new Request("https://cache.test/key"),
  );
  assert.equal(lookup.status, "HIT");
  assert.equal(lookup.fenceGeneration, "2");
  assert.equal(lookup.response.headers.get("content-encoding"), "gzip");
  assert.equal(
    [...lookup.response.headers.keys()].some((key) =>
      key.startsWith("x-open-compute-"),
    ),
    false,
  );
  assert.equal(await lookup.response.text(), "encoded");
});

test("automatic writes pass only the fence fields to the native serializer", async () => {
  const previous = globalThis.caches;
  let called = 0;
  globalThis.caches = {
    default: {
      async put(request, response) {
        called++;
        const context = JSON.parse(request.headers.get(contextHeader));
        assert.deepEqual(context, {
          props,
          fence: { fenceGeneration: "3", refreshToken: "cd".repeat(16) },
        });
        assert.equal(context.props.BINDING_BACKEND_TOKEN, undefined);
        assert.equal(await response.text(), "response");
      },
    },
  };
  try {
    await host({}).putAutomatic(
      new Request("https://cache.test/key"),
      new Response("response"),
      {
        status: "UPDATING",
        fenceGeneration: "3",
        refreshToken: "cd".repeat(16),
        response: new Response("private-lookup"),
      },
    );
    assert.equal(called, 1);
  } finally {
    if (previous === undefined) delete globalThis.caches;
    else globalThis.caches = previous;
  }
});

test("native Cache PUT reaches the same bounded frame and authority", async () => {
  const transport = host({
    async fetch(url, init) {
      assert.ok(url.endsWith("/put"));
      assert.equal(
        init.headers["content-type"],
        "application/vnd.open-compute.cache.v1+frame",
      );
      const bytes = new Uint8Array(await new Response(init.body).arrayBuffer());
      const size = new DataView(bytes.buffer).getUint32(0, false);
      const metadata = JSON.parse(
        new TextDecoder().decode(bytes.subarray(4, size + 4)),
      );
      assert.equal(metadata.namespace, "named");
      assert.equal(metadata.name, "reports:中文 /");
      assert.equal(metadata.method, "GET");
      assert.equal(metadata.status, 200);
      assert.equal(
        new Headers(metadata.responseHeaders).get("content-encoding"),
        "gzip",
      );
      assert.equal(
        metadata.headers.some(([name]) => name.startsWith("x-open-compute-")),
        false,
      );
      assert.equal(new TextDecoder().decode(bytes.subarray(size + 4)), "abc");
      return new Response(null, { status: 204 });
    },
  });
  const input = payload({});
  input.headers.set("cf-cache-namespace", encodeURIComponent("reports:中文 /"));
  assert.equal((await transport.fetch(input)).status, 204);
});

test("native cache failures preserve stable codes and hide raw backend errors", async () => {
  for (const error of [
    new Error("private-upstream-location"),
    Object.assign(new Error("private"), { stableCode: "CACHE_CORRUPT" }),
  ]) {
    const transport = host({
      async fetch() {
        throw error;
      },
    });
    await assert.rejects(
      transport.fetch(new Request("https://cache.test/key")),
      (error) => {
        assert.equal(error.message, error.stableCode);
        assert.equal(error.message, error.stack.slice(7));
        assert.equal(error.message.includes("private"), false);
        return true;
      },
    );
  }
});

test("host serializer validates the descriptor, scope, fence, and encoded payload", async () => {
  let calls = 0;
  const sink = new CacheWriteTransport(
    {
      exports: {
        CacheTransport(options) {
          assert.deepEqual(options, { props });
          return {
            async storeEncoded(namespace, name, request, response, fence) {
              calls++;
              assert.equal(namespace, "automatic");
              assert.equal(name, undefined);
              assert.equal(request.method, "GET");
              assert.equal(request.headers.has(contextHeader), false);
              assert.deepEqual(fence, { fenceGeneration: "4" });
              assert.equal(response.headers.get("content-encoding"), "gzip");
              assert.equal(await response.text(), "abc");
            },
          };
        },
      },
    },
    {},
  );
  const response = await sink.fetch(
    payload({ props, fence: { fenceGeneration: "4" } }),
  );
  assert.equal(response.status, 204);
  assert.equal(calls, 1);
  for (const context of [
    null,
    [],
    {},
    { props },
    { props, fence: { fenceGeneration: "4" }, unexpected: true },
    { props: { ...props, extra: "secret" }, fence: { fenceGeneration: "4" } },
    {
      props: { ...props, descriptorSha256: "wrong" },
      fence: { fenceGeneration: "4" },
    },
    { props, fence: { fenceGeneration: "0" } },
    { props, fence: { fenceGeneration: "4", refreshToken: "wrong" } },
    { props, fence: { fenceGeneration: "4", status: "HIT" } },
  ])
    await assert.rejects(sink.fetch(payload(context)), /CACHE_PROTOCOL_ERROR/);
  await assert.rejects(
    sink.fetch(new Request("https://cache.test/key")),
    /CACHE_PROTOCOL_ERROR/,
  );
  const badJson = payload({});
  badJson.headers.set(contextHeader, "{");
  await assert.rejects(sink.fetch(badJson), /CACHE_PROTOCOL_ERROR/);
  const oversized = payload({});
  oversized.headers.set(contextHeader, "x".repeat(65537));
  await assert.rejects(sink.fetch(oversized), /CACHE_PROTOCOL_ERROR/);
  assert.equal(calls, 1);
});

test("host serializer cancels encoded bytes when the authority rejects a write", async () => {
  let cancellations = 0;
  const sink = new CacheWriteTransport(
    {
      exports: {
        CacheTransport() {
          return {
            async storeEncoded() {
              throw Error("private");
            },
          };
        },
      },
    },
    {},
  );
  const input = new Request("https://cache.test/key", {
    method: "PUT",
    duplex: "half",
    headers: {
      [contextHeader]: JSON.stringify({
        props,
        fence: { fenceGeneration: "4" },
      }),
    },
    body: new ReadableStream({
      start(controller) {
        controller.enqueue(
          new TextEncoder().encode("HTTP/1.1 200 OK\r\n\r\nbody"),
        );
      },
      cancel() {
        cancellations++;
      },
    }),
  });
  await assert.rejects(sink.fetch(input), /^Error: CACHE_PROTOCOL_ERROR$/);
  assert.equal(cancellations, 1);
});

test("Cache transport rejects malformed lookup authority and releases hidden bodies", async () => {
  for (const change of [
    { "x-open-compute-cache-status": "UNKNOWN" },
    { "x-open-compute-cache-fence": "0" },
    { "x-open-compute-cache-status": "UPDATING" },
    { "x-open-compute-cache-refresh-token": "not-canonical" },
    { "x-open-compute-cache-refresh-token": "cd".repeat(16) },
    { "x-open-compute-cache-hit": "0" },
  ]) {
    let cancelled = false;
    const transport = host({
      async fetch() {
        return new Response(
          new ReadableStream({
            cancel() {
              cancelled = true;
            },
          }),
          {
            headers: {
              "x-open-compute-cache-status": "HIT",
              "x-open-compute-cache-fence": "1",
              "x-open-compute-cache-hit": "1",
              ...change,
            },
          },
        );
      },
    });
    await assert.rejects(
      transport.match(
        "default",
        undefined,
        new Request("https://cache.test/key"),
      ),
      /CACHE_PROTOCOL_ERROR/,
    );
    assert.equal(cancelled, true);
  }
  const miss = host({
    async fetch() {
      return new Response(null, {
        status: 204,
        headers: {
          "x-open-compute-cache-status": "MISS",
          "x-open-compute-cache-fence": "1",
          "x-open-compute-cache-hit": "0",
        },
      });
    },
  });
  assert.deepEqual(
    await miss.match(
      "default",
      undefined,
      new Request("https://cache.test/key"),
    ),
    { status: "MISS", fenceGeneration: "1" },
  );
  const inconsistent = host({
    async fetch() {
      return new Response("body", {
        headers: {
          "x-open-compute-cache-status": "MISS",
          "x-open-compute-cache-fence": "1",
          "x-open-compute-cache-hit": "0",
        },
      });
    },
  });
  await assert.rejects(
    inconsistent.match(
      "default",
      undefined,
      new Request("https://cache.test/key"),
    ),
    /CACHE_PROTOCOL_ERROR/,
  );
});

test("Cache mutations validate authority status and JSON before returning results", async () => {
  for (const [method, value] of [
    ["delete", { deleted: true }],
    ["purge", { success: true, deleted: 2 }],
  ]) {
    const transport = host({
      async fetch(url, init) {
        assert.ok(url.endsWith("/" + method));
        assert.equal(init.method, "POST");
        return Response.json(value);
      },
    });
    if (method === "delete")
      assert.equal(
        await transport.delete(
          "named",
          "pages",
          new Request("https://cache.test/key"),
        ),
        true,
      );
    else assert.deepEqual(await transport.purge({ tags: ["release"] }), value);
  }
  for (const value of [
    null,
    { deleted: "true" },
    { deleted: true, private: "value" },
    { success: false, deleted: 2 },
    { success: true, deleted: -1 },
    { success: true, deleted: 1.5 },
  ]) {
    const transport = host({
      async fetch() {
        return Response.json(value);
      },
    });
    await assert.rejects(
      transport.delete(
        "default",
        undefined,
        new Request("https://cache.test/key"),
      ),
      /CACHE_PROTOCOL_ERROR/,
    );
    await assert.rejects(
      transport.purge({ purgeEverything: true }),
      /CACHE_PROTOCOL_ERROR/,
    );
  }
  for (const output of [
    () => new Response(null, { status: 503 }),
    () =>
      new Response(null, {
        status: 500,
        headers: { "x-open-compute-error-code": "CACHE_CORRUPT" },
      }),
    () =>
      new Response(null, {
        status: 500,
        headers: { "x-open-compute-error-code": "private-upstream-location" },
      }),
  ]) {
    const transport = host({
      async fetch() {
        return output();
      },
    });
    await assert.rejects(
      transport.fetch(new Request("https://cache.test/key")),
      (error) => /^(?:CACHE_PROTOCOL_ERROR|CACHE_CORRUPT)$/.test(error.message),
    );
    await assert.rejects(
      transport.delete(
        "default",
        undefined,
        new Request("https://cache.test/key"),
      ),
    );
  }
  const wrongStatus = host({
    async fetch() {
      return new Response(null, { status: 204 });
    },
  });
  await assert.rejects(
    wrongStatus.delete(
      "default",
      undefined,
      new Request("https://cache.test/key"),
    ),
    /CACHE_PROTOCOL_ERROR/,
  );
  const wrongPut = host({
    async fetch() {
      return new Response(null, { status: 200 });
    },
  });
  await assert.rejects(
    wrongPut.storeEncoded(
      "default",
      undefined,
      new Request("https://cache.test/key"),
      new Response("body"),
    ),
    /CACHE_PROTOCOL_ERROR/,
  );
});

test("automatic serialization bounds its context before consuming response bytes", async () => {
  const transport = new CacheTransport(
    { props: { ...props, entrypoint: "x".repeat(65536) } },
    {},
  );
  const response = new Response("unconsumed");
  await assert.rejects(
    transport.putAutomatic(new Request("https://cache.test/key"), response, {
      fenceGeneration: "4",
    }),
    /CACHE_LIMIT_EXCEEDED/,
  );
  assert.equal(response.bodyUsed, false);
});

test("native Cache transport framing never reaches persisted request or response headers", async () => {
  const framing = {
    connection: "keep-alive, X-Transport",
    "keep-alive": "timeout=5",
    "proxy-authenticate": "fixture",
    "proxy-authorization": "fixture",
    te: "trailers",
    trailer: "X-Trailer",
    "transfer-encoding": "chunked",
    upgrade: "fixture",
    "x-transport": "private-hop",
    "x-open-compute-secret": "private-authority",
  };
  let calls = 0;
  const transport = host({
    async fetch(url, init) {
      calls++;
      const bytes = new Uint8Array(await new Response(init.body).arrayBuffer());
      const size = new DataView(bytes.buffer).getUint32(0, false);
      const metadata = JSON.parse(
        new TextDecoder().decode(bytes.subarray(4, 4 + size)),
      );
      assert.deepEqual(metadata.headers, [["accept-language", "en"]]);
      assert.deepEqual(metadata.responseHeaders, [
        ["cache-control", "max-age=120"],
        ["content-type", "text/plain"],
      ]);
      assert.equal(
        new TextDecoder().decode(bytes.subarray(4 + size)),
        "encoded-body",
      );
      assert.equal(metadata.expectedFenceGeneration, "1");
      return new Response(null, { status: 204 });
    },
  });
  await transport.storeEncoded(
    "automatic",
    undefined,
    new Request("https://cache.test/key", {
      headers: { ...framing, "accept-language": "en" },
    }),
    new Response("encoded-body", {
      headers: {
        ...framing,
        "cache-control": "max-age=120",
        "content-type": "text/plain",
      },
    }),
    { fenceGeneration: "1" },
  );
  assert.equal(calls, 1);
});

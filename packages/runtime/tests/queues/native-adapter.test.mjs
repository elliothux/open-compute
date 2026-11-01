import assert from "node:assert/strict";
import test from "node:test";
import { deserialize, serialize } from "node:v8";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";
import { queueFrame } from "./frame.mjs";

const adapterUrl = moduleUrl(
  await compileRuntime("queues/native-adapter.ts", {
    "../bindings/json-body.js": moduleUrl(
      await compileRuntime("bindings/json-body.ts"),
    ),
    "./frame.js": moduleUrl(await compileRuntime("queues/frame.ts")),
    "./metrics.js": moduleUrl(await compileRuntime("queues/metrics.ts")),
  }),
);
const { nativeQueueFetch } = await import(adapterUrl);
const { QueueTransport } = await import(
  moduleUrl(
    await compileRuntime("queues/transport.ts", {
      "cloudflare:workers": moduleUrl(
        "export class WorkerEntrypoint { constructor(ctx,env) { this.ctx=ctx;this.env=env; } }",
      ),
      "../loader/shared.js": moduleUrl(`
    export const bindingError=(code)=>Object.assign(new Error(code),{stableCode:code});
    export const currentStartupGeneration=()=>"generation";`),
      "./native-adapter.js": adapterUrl,
    }),
  )
);
// Node implements the host primitive for adapter unit coverage. The maintained workerd
// queue-wire-codec-test owns actual native V8 decoding; production node:v8 is unavailable.
const wireCodec = { decodeV8: deserialize };
const text = new TextEncoder();
const base64 = (bytes) => Buffer.from(bytes).toString("base64");
function authority(
  metrics = {
    backlogCount: 2,
    backlogBytes: 8,
    oldestMessageTimestampMs: 1000,
  },
) {
  const calls = [];
  return {
    calls,
    async send(frame) {
      calls.push(queueFrame(frame));
      return metrics;
    },
    async sendBatch(frame) {
      calls.push(queueFrame(frame));
      return metrics;
    },
    async metrics() {
      return metrics;
    },
  };
}
const single = (body, headers = {}) =>
  new Request("https://fake-host/message", { method: "POST", body, headers });
const batch = (messages, headers = {}) =>
  new Request("https://fake-host/batch", {
    method: "POST",
    body: JSON.stringify({ messages }),
    headers,
  });

test("Queue transport pins authority, retains outbox operations and denies unqualified native DO sends", async () => {
  const calls = [];
  const props = {
    bindingId: "binding",
    versionId: "version",
    queueId: "queue",
    descriptorSha256: "a".repeat(64),
    queueLifecycleGeneration: 1,
    durableObject: false,
  };
  const env = {
    QUEUE_WIRE_CODEC: wireCodec,
    BINDING_BACKEND: {
      async fetch(url, init) {
        calls.push({ url, ...init });
        return Response.json({ backlogCount: 1, backlogBytes: 4 });
      },
    },
  };
  const transport = new QueueTransport({ props }, env);
  const response = await transport.fetch(
    single("text", { "x-msg-fmt": "text" }),
  );
  assert.equal(response.status, 200);
  assert.equal(calls.length, 1);
  assert.equal(
    calls[0].url,
    "http://binding-backend/internal/bindings/v1/queue/binding/send",
  );
  assert.equal(calls[0].headers["x-open-compute-binding-token"], undefined);
  assert.equal(calls[0].headers["x-open-compute-version-id"], "version");
  assert.equal(
    calls[0].headers["x-open-compute-descriptor-sha256"],
    "a".repeat(64),
  );
  assert.equal(
    calls[0].headers["x-open-compute-startup-generation"],
    undefined,
  );
  assert.equal(calls[0].headers["x-open-compute-output-gate"], "0");
  assert.equal(queueFrame(calls[0].body).messages[0].contentType, 2);
  for (const durableObject of [true, undefined, "false"]) {
    const denied = new QueueTransport(
      { props: { ...props, durableObject } },
      env,
    );
    await error(
      await denied.fetch(single("text")),
      "QUEUE_INVARIANT_VIOLATION",
    );
  }
  assert.equal(calls.length, 1);
  const doTransport = new QueueTransport(
    { props: { ...props, durableObject: true } },
    env,
  );
  const operation = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
  await doTransport.send(calls[0].body, operation);
  await doTransport.sendBatch(calls[0].body, operation);
  await doTransport.finalize(operation);
  for (const call of calls.slice(1)) {
    assert.equal(call.headers["x-open-compute-output-gate"], "1");
    assert.equal(call.headers["x-open-compute-request-id"], operation);
  }
  assert.ok(calls[3].url.endsWith("/finalize"));
  assert.equal(calls[3].body, undefined);
  await assert.rejects(doTransport.send(calls[0].body, "bad-id"), {
    message: "QUEUE_INVARIANT_VIOLATION",
  });
  assert.equal(calls.length, 4);
});
async function error(response, expected) {
  assert.equal(response.status, 500);
  assert.equal(response.headers.get("cf-queues-error-cause"), expected);
  assert.equal(response.headers.get("cf-queues-error-code"), "15000");
  assert.equal(await response.text(), expected);
}

test("native Queue wire preserves formats, graph values, delays and timestamp metadata", async () => {
  const raw = authority();
  for (const [body, format, code] of [
    [text.encode('{"v":1}'), "json", 1],
    [text.encode("日本語"), "text", 2],
    [new Uint8Array([0, 1, 255]), "bytes", 3],
  ]) {
    const response = await nativeQueueFetch(
      single(body, { "x-msg-fmt": format, "x-msg-delay-secs": "9" }),
      raw,
      wireCodec,
    );
    assert.deepEqual(await response.json(), {
      metadata: {
        metrics: {
          backlogCount: 2,
          backlogBytes: 8,
          oldestMessageTimestamp: 1000,
        },
      },
    });
    assert.deepEqual(raw.calls.at(-1), {
      operation: 1,
      delay: -1,
      messages: [{ contentType: code, delay: 9, body }],
    });
  }
  const graph = {
    date: new Date(42),
    big: 12n,
    map: new Map([["k", new Set([1])]]),
    bytes: new Uint8Array([2, 3]),
  };
  graph.self = graph;
  for (const headers of [{}, { "x-msg-fmt": "v8" }]) {
    const response = await nativeQueueFetch(
      single(serialize(graph), headers),
      raw,
      wireCodec,
    );
    assert.equal(response.status, 200);
    const message = raw.calls.at(-1).messages[0];
    assert.equal(message.contentType, 4);
    const restored = deserialize(message.body);
    assert.deepEqual(restored, graph);
    assert.equal(restored.self, restored);
  }
  assert.deepEqual(
    await (
      await nativeQueueFetch(
        new Request("https://fake-host/metrics"),
        raw,
        wireCodec,
      )
    ).json(),
    {
      backlogCount: 2,
      backlogBytes: 8,
      oldestMessageTimestamp: 1000,
    },
  );
  for (const oldest of [null, undefined, 0]) {
    const response = await nativeQueueFetch(
      new Request("https://fake-host/metrics"),
      authority({
        backlogCount: 0,
        backlogBytes: 0,
        oldestMessageTimestampMs: oldest,
      }),
      wireCodec,
    );
    assert.equal((await response.json()).oldestMessageTimestamp, 0);
  }
});

test("native Queue batch validates all messages and accounting before one authority mutation", async () => {
  const raw = authority();
  const values = [
    { body: base64(text.encode("null")), contentType: "json" },
    { body: base64(text.encode("text")), contentType: "text", delaySecs: 0 },
    { body: base64(new Uint8Array([9])), contentType: "bytes", delaySecs: 4 },
    { body: base64(serialize(12n)) },
  ];
  const lengths = values.map(
    (value) => Buffer.from(value.body, "base64").length,
  );
  const response = await nativeQueueFetch(
    batch(values, {
      "x-msg-delay-secs": "3",
      "cf-queue-batch-count": "4",
      "cf-queue-batch-bytes": String(lengths.reduce((a, b) => a + b)),
      "cf-queue-largest-msg": String(Math.max(...lengths)),
    }),
    raw,
    wireCodec,
  );
  assert.equal(response.status, 200);
  assert.equal(raw.calls.length, 1);
  assert.equal(raw.calls[0].operation, 2);
  assert.equal(raw.calls[0].delay, 3);
  assert.deepEqual(
    raw.calls[0].messages.map((item) => [item.contentType, item.delay]),
    [
      [1, -1],
      [2, 0],
      [3, 4],
      [4, -1],
    ],
  );
  assert.equal(deserialize(raw.calls[0].messages[3].body), 12n);
  raw.calls.length = 0;
  for (const headers of [
    { "cf-queue-batch-count": "3" },
    { "cf-queue-batch-bytes": "1" },
    { "cf-queue-largest-msg": "0" },
  ])
    await error(
      await nativeQueueFetch(batch(values, headers), raw, wireCodec),
      "QUEUE_INVALID_MESSAGE",
    );
  for (const item of [
    null,
    {},
    { body: "%%%" },
    { body: "YQ" },
    { body: "YQ==", contentType: null },
    { body: "YQ==", contentType: "xml" },
    { body: "YQ==", contentType: "text", extra: "private" },
    { body: "YQ==", contentType: "text", delaySecs: 86401 },
  ]) {
    const response = await nativeQueueFetch(
      batch([values[0], item]),
      raw,
      wireCodec,
    );
    assert.equal(response.status, 500);
    assert.doesNotMatch(await response.text(), /private/);
  }
  assert.equal(raw.calls.length, 0);
});

test("native Queue rejects malformed wire and hard limits without publishing a prefix", async () => {
  const raw = authority();
  for (const [request, code] of [
    [
      new Request("https://other/message", { method: "POST" }),
      "QUEUE_INVALID_MESSAGE",
    ],
    [
      new Request("http://fake-host/message", { method: "POST" }),
      "QUEUE_INVALID_MESSAGE",
    ],
    [
      new Request("https://fake-host/message?x=1", { method: "POST" }),
      "QUEUE_INVALID_MESSAGE",
    ],
    [
      new Request("https://fake-host/unknown", { method: "POST" }),
      "QUEUE_INVALID_MESSAGE",
    ],
    [new Request("https://fake-host/message"), "QUEUE_INVALID_MESSAGE"],
    [single("{}", { "x-msg-fmt": "xml" }), "QUEUE_CONTENT_TYPE_UNSUPPORTED"],
    [single("{", { "x-msg-fmt": "json" }), "QUEUE_INVALID_MESSAGE"],
    [
      single(new Uint8Array([255]), { "x-msg-fmt": "text" }),
      "QUEUE_INVALID_MESSAGE",
    ],
    [single(new Uint8Array([255, 15])), "QUEUE_V8_MALFORMED"],
    [single(serialize(undefined)), "QUEUE_INVALID_MESSAGE"],
    [
      single(new Uint8Array(128001), { "x-msg-fmt": "bytes" }),
      "QUEUE_MESSAGE_TOO_LARGE",
    ],
    [batch([]), "QUEUE_BATCH_LIMIT_EXCEEDED"],
    [
      batch(
        Array.from({ length: 101 }, () => ({
          body: "YQ==",
          contentType: "text",
        })),
      ),
      "QUEUE_BATCH_LIMIT_EXCEEDED",
    ],
    [
      new Request("https://fake-host/batch", { method: "POST", body: "{" }),
      "QUEUE_BATCH_LIMIT_EXCEEDED",
    ],
    [
      new Request("https://fake-host/batch", {
        method: "POST",
        body: "x".repeat(360001),
      }),
      "QUEUE_BATCH_LIMIT_EXCEEDED",
    ],
  ])
    await error(await nativeQueueFetch(request, raw, wireCodec), code);
  for (const delay of ["-1", "86401", "01", "1.0", "1e1", "a", "100000"]) {
    await error(
      await nativeQueueFetch(
        single("a", { "x-msg-fmt": "text", "x-msg-delay-secs": delay }),
        raw,
        wireCodec,
      ),
      "QUEUE_DELAY_INVALID",
    );
  }
  const big = { body: base64(new Uint8Array(90000)), contentType: "bytes" };
  await error(
    await nativeQueueFetch(batch([big, big, big]), raw, wireCodec),
    "QUEUE_BATCH_LIMIT_EXCEEDED",
  );
  assert.equal(raw.calls.length, 0);
});

test("native Queue bounds stream consumption, cancels overflow and sanitizes authority failures", async () => {
  const raw = authority();
  let cancelled = false;
  const stream = new ReadableStream({
    pull(controller) {
      controller.enqueue(new Uint8Array(128001));
    },
    cancel() {
      cancelled = true;
    },
  });
  const request = new Request("https://fake-host/message", {
    method: "POST",
    body: stream,
    duplex: "half",
    headers: { "x-msg-fmt": "bytes" },
  });
  await error(
    await nativeQueueFetch(request, raw, wireCodec),
    "QUEUE_MESSAGE_TOO_LARGE",
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(cancelled, true);
  assert.equal(raw.calls.length, 0);
  for (const metrics of [
    null,
    {},
    { backlogCount: -1, backlogBytes: 0 },
    { backlogCount: 0, backlogBytes: NaN },
    { backlogCount: 0, backlogBytes: 0, oldestMessageTimestampMs: "secret" },
    {
      backlogCount: 0,
      backlogBytes: 0,
      oldestMessageTimestampMs: Number.MAX_SAFE_INTEGER,
    },
  ]) {
    await error(
      await nativeQueueFetch(
        new Request("https://fake-host/metrics"),
        authority(metrics),
        wireCodec,
      ),
      "QUEUE_INVARIANT_VIOLATION",
    );
  }
  for (const code of [
    "QUEUE_BACKLOG_LIMIT_EXCEEDED",
    "QUEUE_SEND_RESULT_UNKNOWN",
    "BINDING_PERMISSION_DENIED",
    "PRIVATE_PATH_TOKEN",
  ]) {
    const response = await nativeQueueFetch(
      single("text", { "x-msg-fmt": "text" }),
      {
        async send() {
          throw new Error(code);
        },
      },
      wireCodec,
    );
    await error(
      response,
      code === "PRIVATE_PATH_TOKEN" ? "QUEUE_STORAGE_UNAVAILABLE" : code,
    );
  }
});

test("native V8 messages keep exact wire bytes and use native single and batch budgets", async () => {
  const raw = authority();
  const value = "你".repeat(50_000);
  const body = serialize(value);
  assert.equal(body.byteLength, 100006);
  const accepted = await nativeQueueFetch(single(body), raw, wireCodec);
  assert.equal(accepted.status, 200);
  assert.deepEqual(raw.calls[0].messages[0].body, new Uint8Array(body));
  assert.equal(deserialize(raw.calls[0].messages[0].body), value);
  const exact = serialize("a".repeat(127994));
  assert.equal(exact.byteLength, 128000);
  assert.equal(
    (await nativeQueueFetch(single(exact), raw, wireCodec)).status,
    200,
  );
  assert.deepEqual(raw.calls[1].messages[0].body, new Uint8Array(exact));
  await error(
    await nativeQueueFetch(
      single(serialize("a".repeat(127995))),
      raw,
      wireCodec,
    ),
    "QUEUE_MESSAGE_TOO_LARGE",
  );
  const messages = [
    { body: base64(body) },
    { body: base64(body) },
    {
      body: base64(new Uint8Array(256000 - body.byteLength * 2)),
      contentType: "bytes",
    },
  ];
  const result = await nativeQueueFetch(batch(messages), raw, wireCodec);
  assert.equal(result.status, 200);
  assert.equal(raw.calls.length, 3);
  assert.equal(
    raw.calls[2].messages.reduce(
      (sum, message) => sum + message.body.length,
      0,
    ),
    256000,
  );
  messages[2].body = base64(new Uint8Array(256001 - body.byteLength * 2));
  await error(
    await nativeQueueFetch(batch(messages), raw, wireCodec),
    "QUEUE_BATCH_LIMIT_EXCEEDED",
  );
  assert.equal(raw.calls.length, 3);
});

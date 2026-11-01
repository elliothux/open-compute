import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const base = moduleUrl(
  "export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }",
);
const host = moduleUrl(
  "export const bindingError = code => Object.assign(new Error(code), { stableCode: code }); export const currentStartupGeneration = () => 'generation';",
);
const jsonBody = moduleUrl(await compileRuntime("bindings/json-body.ts"));
const native = moduleUrl(
  await compileRuntime("kv/native-adapter.ts", {
    "../bindings/json-body.js": jsonBody,
  }),
);
const { KVNamespace } = await importRuntime("kv/transport.ts", {
  "cloudflare:workers": base,
  "../loader/shared.js": host,
  "./native-adapter.js": native,
});
const contentType = "application/vnd.open-compute.kv.v1+frame";
const props = {
  bindingId: "binding",
  versionId: "version",
  descriptorSha256: "a".repeat(64),
  resourceSpecGeneration: 1,
  permissions: { read: true, write: true },
};
const transport = (fetch, permissions = props.permissions) =>
  new KVNamespace(
    { props: { ...props, permissions } },
    { BINDING_BACKEND: { fetch } },
  );

test("native KV bulk rejects JSON expansion above the response budget", async () => {
  const value = "\u0000".repeat(5 * 1024 * 1024);
  const kv = transport(async () => bulk([{ value, metadata: null }]));
  const response = await kv.fetch(
    new Request("https://fake-host/bulk/get", {
      method: "POST",
      body: JSON.stringify({ keys: ["control"] }),
    }),
  );
  assert.equal(response.status, 413);
  assert.equal(response.statusText, "KV_BULK_TOO_LARGE");
  assert.equal(await response.text(), "");
});

test("native KV bulk counts escaped UTF-8, JSON containers and metadata at the exact limit", async () => {
  const maximum = 25 * 1024 * 1024;
  const key = 'control\n"\\你😀';
  const special = '\b\t\n\f\r\u0000"\\aé你😀\ud800x\udc00';
  for (const withMetadata of [false, true]) {
    const metadata = {
      nested: [true, false, null, 1.25, { [special]: special }],
    };
    const value = (padding) =>
      withMetadata
        ? [padding, { special, missing: null }]
        : padding + '\b\t\n\f\r\u0000"\\aé你😀';
    const output = (padding) => ({
      [key]: withMetadata
        ? { value: value(padding), metadata }
        : value(padding),
    });
    const available = maximum - Buffer.byteLength(JSON.stringify(output("")));
    const padding =
      "\u0000".repeat(Math.floor(available / 6)) + "a".repeat(available % 6);
    for (const extra of ["", "a"]) {
      const expected = output(padding + extra);
      const stored = value(padding + extra);
      const kv = transport(async () =>
        bulk([
          {
            value: withMetadata ? JSON.stringify(stored) : stored,
            metadata,
          },
        ]),
      );
      const response = await kv.fetch(
        new Request("https://fake-host/bulk/get", {
          method: "POST",
          body: JSON.stringify({
            keys: [key],
            type: withMetadata ? "json" : "text",
            withMetadata,
          }),
        }),
      );
      if (extra) {
        assert.equal(response.status, 413);
        assert.equal(response.statusText, "KV_BULK_TOO_LARGE");
        assert.equal(await response.text(), "");
      } else {
        assert.equal(response.status, 200);
        assert.equal(response.headers.get("content-type"), "application/json");
        const body = await response.text();
        assert.equal(Buffer.byteLength(body), maximum);
        assert.deepEqual(JSON.parse(body), expected);
      }
    }
  }
});

test("native KV wire reuses authority, streams values and sanitizes failure responses", async () => {
  const calls = [];
  const kv = transport(async (url, options) => {
    const operation = url.split("/").at(-1);
    if (operation === "put") {
      const bytes = Buffer.from(await new Response(options.body).arrayBuffer());
      const length = bytes.readUInt32BE();
      calls.push({
        operation,
        header: JSON.parse(bytes.subarray(4, 4 + length)),
        value: bytes.subarray(4 + length).toString(),
      });
      return new Response(null, { status: 204 });
    }
    const input = JSON.parse(options.body);
    calls.push({ operation, input });
    if (operation === "get-with-metadata")
      return input.keys[0] === "missing"
        ? result(null)
        : result("value", {
            owner: "tenant",
          });
    if (operation === "get-many")
      return bulk(
        input.keys.map((key) =>
          key === "missing" ? null : { value: '{"v":1}', metadata: { key } },
        ),
      );
    if (operation === "list")
      return Response.json({
        keys: [{ name: "key", expiration: null, metadata: { value: 1 } }],
        list_complete: true,
        cursor: null,
      });
    assert.equal(operation, "delete");
    return new Response(null, { status: 204 });
  });
  const fetch = (path, options) =>
    kv.fetch(new Request(`https://fake-host${path}`, options));
  const read = await fetch(
    "/nested%2F%E4%BD%A0%25?urlencoded=true&cache_ttl=30",
  );
  assert.equal(await read.text(), "value");
  assert.deepEqual(JSON.parse(read.headers.get("cf-kv-metadata")), {
    owner: "tenant",
  });
  assert.deepEqual(calls.at(-1).input, { keys: ["nested/你%"], cacheTtl: 30 });
  assert.equal((await fetch("/missing?urlencoded=true")).status, 404);
  assert.equal(
    (
      await fetch("/key?urlencoded=true&expiration_ttl=60", {
        method: "PUT",
        headers: { "cf-kv-metadata": '{"m":1}' },
        body: "streamed",
      })
    ).status,
    204,
  );
  assert.equal(calls.at(-1).value, "streamed");
  assert.equal(calls.at(-1).header.expirationTtl, 60);
  assert.deepEqual(calls.at(-1).header.metadata, { m: 1 });
  const expiration = Math.floor(Date.now() / 1000) + 120;
  assert.equal(
    (
      await fetch(`/key?urlencoded=true&expiration=${expiration}`, {
        method: "PUT",
      })
    ).status,
    204,
  );
  assert.equal(calls.at(-1).header.expiration, expiration);
  assert.equal(calls.at(-1).value, "");
  assert.equal(
    (await fetch("/key?urlencoded=true", { method: "DELETE" })).status,
    204,
  );
  assert.deepEqual(calls.at(-1).input, { key: "key" });
  const listed = await (
    await fetch("/?prefix=k&key_count_limit=7&cursor=c")
  ).json();
  assert.equal(listed.keys[0].metadata, '{"value":1}');
  assert.deepEqual(calls.at(-1).input, { prefix: "k", limit: 7, cursor: "c" });
  const readMany = (input) =>
    fetch("/bulk/get", { method: "POST", body: JSON.stringify(input) });
  assert.deepEqual(
    await (
      await readMany({ keys: ["key", "missing"], type: "json", cacheTtl: "30" })
    ).json(),
    { key: { v: 1 }, missing: null },
  );
  assert.deepEqual(
    await (
      await readMany({ keys: ["key", "missing"], withMetadata: true })
    ).json(),
    { key: { value: '{"v":1}', metadata: { key: "key" } }, missing: null },
  );
  for (const input of [
    null,
    {},
    { keys: [4] },
    { keys: ["k"], type: "stream" },
    { keys: ["k"], withMetadata: "yes" },
    { keys: [] },
    { keys: ["k"], cacheTtl: "bad" },
  ]) {
    assert.equal((await readMany(input)).status, 400);
  }
  assert.equal((await readMany({ keys: ["x".repeat(65 * 1024)] })).status, 400);
  assert.equal((await fetch("/key")).status, 400);
  assert.equal(
    (await fetch("/key?urlencoded=true", { method: "PATCH" })).status,
    400,
  );
  assert.equal(
    (await kv.fetch(new Request("https://other/key?urlencoded=true"))).status,
    400,
  );
  const failed = transport(async () => {
    throw new Error("private token=hidden");
  });
  const sanitized = await failed.fetch(
    new Request("https://fake-host/key?urlencoded=true"),
  );
  assert.equal(sanitized.statusText, "KV_INTERNAL_PROTOCOL_ERROR");
  assert.equal(await sanitized.text(), "");
  const uppercase = transport(async () => {
    throw new Error("KV_PRIVATE_SECRET");
  });
  assert.equal(
    (
      await uppercase.fetch(
        new Request("https://fake-host/key?urlencoded=true"),
      )
    ).statusText,
    "KV_INTERNAL_PROTOCOL_ERROR",
  );
});

function header(valueLength, metadata = null) {
  const encoded =
    metadata === null ? null : Buffer.from(JSON.stringify(metadata));
  const bytes = Buffer.alloc(21 + (encoded?.length ?? 0));
  bytes.write("KVS1");
  bytes[4] = valueLength === null ? 0 : 1;
  bytes.writeBigInt64BE(-1n, 5);
  bytes.writeUInt32BE(encoded?.length ?? 0xffffffff, 13);
  encoded?.copy(bytes, 17);
  bytes.writeUInt32BE(valueLength ?? 0xffffffff, 17 + (encoded?.length ?? 0));
  return bytes;
}

function result(value, metadata = null) {
  const bytes = value === null ? null : Buffer.from(value);
  return new Response(
    Buffer.concat([
      header(bytes?.length ?? null, metadata),
      bytes ?? Buffer.alloc(0),
    ]),
    {
      headers: { "content-type": contentType },
    },
  );
}

function bulkEntry(value, metadata = null) {
  const encoded =
    metadata === null ? null : Buffer.from(JSON.stringify(metadata));
  const valueBytes = value === null ? null : Buffer.from(value);
  const bytes = Buffer.alloc(
    17 + (encoded?.length ?? 0) + (valueBytes?.length ?? 0),
  );
  bytes[0] = valueBytes === null ? 0 : 1;
  bytes.writeBigInt64BE(-1n, 1);
  bytes.writeUInt32BE(encoded?.length ?? 0xffffffff, 9);
  encoded?.copy(bytes, 13);
  const valueOffset = 13 + (encoded?.length ?? 0);
  bytes.writeUInt32BE(valueBytes?.length ?? 0xffffffff, valueOffset);
  valueBytes?.copy(bytes, valueOffset + 4);
  return bytes;
}

function bulk(entries) {
  const chunks = [Buffer.from("KVB1")];
  const count = Buffer.alloc(2);
  count.writeUInt16BE(entries.length);
  chunks.push(count);
  for (const entry of entries) {
    chunks.push(
      entry === null
        ? bulkEntry(null)
        : bulkEntry(entry.value, entry.metadata ?? null),
    );
  }
  return new Response(Buffer.concat(chunks), {
    headers: { "content-type": contentType },
  });
}

test("KV uses one frame protocol for default text, binary, JSON and metadata reads", async () => {
  const calls = [];
  const kv = transport(async (url, options) => {
    assert.equal(options.headers["content-type"], contentType);
    assert.equal(
      options.headers["x-open-compute-startup-generation"],
      undefined,
    );
    calls.push({
      operation: url.split("/").at(-1),
      request: JSON.parse(options.body),
    });
    return result('{"ok":true}', { owner: "app" });
  });
  assert.equal(await kv.get("key"), '{"ok":true}');
  assert.deepEqual(await kv.get("key", "json"), { ok: true });
  assert.deepEqual(
    Buffer.from(await kv.get("key", "arrayBuffer")),
    Buffer.from('{"ok":true}'),
  );
  assert.deepEqual(await kv.get("key", { type: "text" }), '{"ok":true}');
  assert.deepEqual(
    await kv.getWithMetadata("key", { type: "json", cacheTtl: 30 }),
    {
      value: { ok: true },
      metadata: { owner: "app" },
      cacheStatus: null,
    },
  );
  assert.deepEqual(calls.at(-1), {
    operation: "get-with-metadata",
    request: { keys: ["key"], cacheTtl: 30 },
  });
  assert.equal(await transport(async () => result(null)).get("missing"), null);
  assert.deepEqual(
    await transport(async () => result(null)).getWithMetadata("missing"),
    {
      value: null,
      metadata: null,
      cacheStatus: null,
    },
  );
  assert.deepEqual(
    new Uint8Array(
      await transport(async () => result([0, 255])).get(
        "binary",
        "arrayBuffer",
      ),
    ),
    new Uint8Array([0, 255]),
  );
});

test("KV bulk get and getWithMetadata preserve the upstream Map value shape", async () => {
  let response;
  const kv = transport(async (url) => {
    assert.equal(url.split("/").at(-1), "get-many");
    response = bulk([
      { value: '{"ok":true}', metadata: { a: 1 } },
      null,
      { value: '{"ok":true}', metadata: { a: 1 } },
    ]);
    return response;
  });
  assert.deepEqual(
    [...(await kv.get(["one", "missing", "one"], "json"))],
    [
      ["one", { ok: true }],
      ["missing", null],
    ],
  );
  assert.equal(response.body.locked, false);
  assert.deepEqual(
    [
      ...(await kv.getWithMetadata(["one", "missing", "one"], {
        type: "json",
      })),
    ],
    [
      ["one", { value: { ok: true }, metadata: { a: 1 } }],
      ["missing", null],
    ],
  );
  assert.equal(response.body.locked, false);
});

test("KV streams propagate cancellation to the backend without buffering the value", async () => {
  let pulled = 0;
  let cancelled = false;
  const kv = transport(
    async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(header(100_000));
          },
          pull(controller) {
            pulled++;
            controller.enqueue(new Uint8Array([1]));
          },
          cancel() {
            cancelled = true;
          },
        }),
        { headers: { "content-type": contentType } },
      ),
  );
  const reader = (await kv.get("key", "stream")).getReader();
  assert.deepEqual((await reader.read()).value, new Uint8Array([1]));
  await reader.cancel("consumer stopped");
  assert.equal(cancelled, true);
  assert.ok(pulled < 10, `unexpected eager reads: ${pulled}`);
});

test("KV binary and stream writes use length-framed metadata and raw bytes", async () => {
  const calls = [];
  const kv = transport(async (url, options) => {
    assert.equal(options.headers["content-type"], contentType);
    calls.push({
      operation: url.split("/").at(-1),
      bytes: Buffer.from(await new Response(options.body).arrayBuffer()),
    });
    return new Response(null, { status: 204 });
  });
  for (const value of [
    new Uint8Array([0, 255]),
    new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array([0]));
        controller.enqueue(new Uint8Array([255]));
        controller.close();
      },
    }),
  ]) {
    await kv.put("key", value, { metadata: { ok: true }, expirationTtl: 60 });
    const { operation, bytes } = calls.at(-1);
    assert.equal(operation, "put");
    const length = bytes.readUInt32BE(0);
    assert.deepEqual(JSON.parse(bytes.subarray(4, 4 + length)), {
      key: "key",
      metadata: { ok: true },
      metadataPresent: true,
      expirationTtl: 60,
    });
    assert.deepEqual(bytes.subarray(4 + length), Buffer.from([0, 255]));
  }
  await kv.delete("key");
  assert.deepEqual(JSON.parse(calls.at(-1).bytes), { key: "key" });
});

test("KV copies resizable buffers before await and rejects detached views", async () => {
  const calls = [];
  const kv = transport(async (_url, options) => {
    calls.push(Buffer.from(await new Response(options.body).arrayBuffer()));
    return new Response(null, { status: 204 });
  });
  const resizable = new ArrayBuffer(4, { maxByteLength: 16 });
  new Uint8Array(resizable).set([9, 8, 7, 6]);
  const pending = kv.put("rab", resizable);
  resizable.resize(0);
  await pending;
  const bytes = calls.at(-1);
  const length = bytes.readUInt32BE(0);
  assert.deepEqual(bytes.subarray(4 + length), Buffer.from([9, 8, 7, 6]));

  const detached = new ArrayBuffer(4);
  if (typeof detached.transfer === "function") detached.transfer();
  else structuredClone(detached, { transfer: [detached] });
  await assert.rejects(
    kv.put("gone", detached),
    /KV value must be a string, buffer, view, or ReadableStream/,
  );
  if (typeof SharedArrayBuffer === "function") {
    const shared = new SharedArrayBuffer(3);
    const view = new Uint8Array(shared);
    view.set([1, 2, 3]);
    await kv.put("sab", view);
    view.set([9, 9, 9]);
    const sabBytes = calls.at(-1);
    const sabLength = sabBytes.readUInt32BE(0);
    assert.deepEqual(sabBytes.subarray(4 + sabLength), Buffer.from([1, 2, 3]));
  }
});

test("KV list accepts null prefix/cursor and returns the discriminated cacheStatus shape", async () => {
  const calls = [];
  const kv = transport(async (_url, options) => {
    calls.push(JSON.parse(options.body));
    if (calls.length === 1) {
      return Response.json({
        keys: [{ name: "a", expiration: 100, metadata: { z: 1 } }],
        list_complete: false,
        cursor: "next",
      });
    }
    return Response.json({ keys: [], list_complete: true, cursor: null });
  });
  assert.deepEqual(await kv.list({ prefix: null, cursor: null, limit: 1 }), {
    keys: [{ name: "a", expiration: 100, metadata: { z: 1 } }],
    list_complete: false,
    cursor: "next",
    cacheStatus: null,
  });
  assert.deepEqual(await kv.list({ prefix: null, cursor: null }), {
    keys: [],
    list_complete: true,
    cacheStatus: null,
  });
  assert.equal("cursor" in (await kv.list()), false);
  assert.deepEqual(calls[0], { prefix: "", limit: 1, cursor: null });
  assert.deepEqual(calls[1], { prefix: "", limit: 1000, cursor: null });
});

test("KV validates keys, bulk size, options, UTF-16 and JSON locally", async () => {
  const kv = transport(async () => {
    throw new Error("must not reach backend");
  });
  await assert.rejects(kv.get(""), {
    name: "TypeError",
    message: "KV_KEY_INVALID",
  });
  await assert.rejects(kv.get("."), {
    name: "TypeError",
    message: "KV_KEY_INVALID",
  });
  await assert.rejects(kv.get(".."), {
    name: "TypeError",
    message: "KV_KEY_INVALID",
  });
  await assert.rejects(kv.get("\uD800"), {
    name: "TypeError",
    message: "KV_KEY_INVALID",
  });
  await assert.rejects(kv.get([]), {
    name: "TypeError",
    message: "KV_TOO_MANY_KEYS",
  });
  await assert.rejects(kv.get(Array.from({ length: 101 }, (_, i) => `k${i}`)), {
    name: "TypeError",
    message: "KV_TOO_MANY_KEYS",
  });
  await assert.rejects(kv.get("k", "banana"), {
    name: "TypeError",
    message: "KV_INVALID_OPTIONS",
  });
  await assert.rejects(kv.get(["k"], "arrayBuffer"), {
    name: "TypeError",
    message: "KV_INVALID_OPTIONS",
  });
  await assert.rejects(kv.get("k", { cacheTtl: 29 }), {
    name: "TypeError",
    message: "KV_INVALID_OPTIONS",
  });
  await assert.rejects(
    kv.put("k", "v", { expiration: 10, expirationTtl: 60 }),
    { name: "TypeError", message: "KV_INVALID_OPTIONS" },
  );
  await assert.rejects(kv.put("k", "v", { expirationTtl: 59 }), {
    name: "TypeError",
    message: "KV_INVALID_OPTIONS",
  });
  await assert.rejects(
    kv.put("k", {}),
    /KV value must be a string, buffer, view, or ReadableStream/,
  );
  await assert.rejects(kv.list({ prefix: 1 }), {
    name: "TypeError",
    message: "KV_KEY_INVALID",
  });
  await assert.rejects(kv.list({ limit: 0 }), {
    name: "TypeError",
    message: "KV_INVALID_OPTIONS",
  });
  await assert.rejects(kv.list({ extra: true }), {
    name: "TypeError",
    message: "KV_INVALID_OPTIONS",
  });
});

test("KV malformed JSON rejects without leaking protocol bytes", async () => {
  const kv = transport(async () => result("{", null));
  await assert.rejects(kv.get("k", "json"), SyntaxError);
  const corrupt = transport(async () => {
    const metadata = Buffer.from("{");
    const bytes = Buffer.alloc(21 + metadata.length);
    bytes.write("KVS1");
    bytes[4] = 1;
    bytes.writeBigInt64BE(-1n, 5);
    bytes.writeUInt32BE(metadata.length, 13);
    metadata.copy(bytes, 17);
    bytes.writeUInt32BE(2, 17 + metadata.length);
    return new Response(Buffer.concat([bytes, Buffer.from("ab")]), {
      headers: { "content-type": contentType },
    });
  });
  await assert.rejects(corrupt.getWithMetadata("k"), {
    message: "KV_INTERNAL_PROTOCOL_ERROR",
  });
});

function hanging(bytes) {
  let cancelled = false;
  let response;
  return {
    cancelled: () => cancelled,
    locked: () => response.body.locked,
    fetch: async () => {
      response = new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(Buffer.from(bytes));
          },
          cancel() {
            cancelled = true;
          },
        }),
        { headers: { "content-type": contentType } },
      );
      return response;
    },
  };
}

function singleFields({
  found = 1,
  expiration = -1n,
  metadataLength,
  valueLength,
}) {
  const bytes = Buffer.alloc(21);
  bytes.write("KVS1");
  bytes[4] = found;
  bytes.writeBigInt64BE(expiration, 5);
  bytes.writeUInt32BE(metadataLength, 13);
  bytes.writeUInt32BE(valueLength, 17);
  return bytes;
}

function bulkFields({
  found = 1,
  expiration = -1n,
  metadataLength,
  valueLength,
}) {
  const prefix = Buffer.alloc(6);
  prefix.write("KVB1");
  prefix.writeUInt16BE(1, 4);
  const entry = Buffer.alloc(17);
  entry[0] = found;
  entry.writeBigInt64BE(expiration, 1);
  entry.writeUInt32BE(metadataLength, 9);
  entry.writeUInt32BE(valueLength, 13);
  return Buffer.concat([prefix, entry]);
}

test("KV rejects non-canonical frames and cancels the backend reader", async () => {
  const cases = [
    [
      "found marker 2",
      singleFields({ found: 2, metadataLength: 0xffffffff, valueLength: 1 }),
    ],
    [
      "found marker 255",
      singleFields({ found: 255, metadataLength: 0xffffffff, valueLength: 1 }),
    ],
    [
      "missing with metadata length",
      singleFields({ found: 0, metadataLength: 2, valueLength: 0xffffffff }),
    ],
    [
      "missing with expiration",
      singleFields({
        found: 0,
        expiration: 1n,
        metadataLength: 0xffffffff,
        valueLength: 0xffffffff,
      }),
    ],
    [
      "missing with value length",
      singleFields({ found: 0, metadataLength: 0xffffffff, valueLength: 0 }),
    ],
    [
      "unsafe metadata length",
      singleFields({ found: 1, metadataLength: 1025, valueLength: 1 }),
    ],
    [
      "unsafe value length",
      singleFields({
        found: 1,
        metadataLength: 0xffffffff,
        valueLength: 25 * 1024 * 1024 + 1,
      }),
    ],
    [
      "trailing bytes after missing",
      Buffer.concat([header(null), Buffer.from([1])]),
    ],
  ];
  for (const [label, bytes] of cases) {
    const hung = hanging(bytes);
    const kv = transport(hung.fetch);
    await assert.rejects(
      kv.get("k"),
      { message: "KV_INTERNAL_PROTOCOL_ERROR" },
      label,
    );
    assert.equal(hung.cancelled(), true, `${label} must cancel`);
    assert.equal(hung.locked(), false, `${label} must release the body lock`);
  }
  let truncatedBody;
  const truncated = transport(async () => {
    truncatedBody = new Response(Buffer.from("KVS1"), {
      headers: { "content-type": contentType },
    });
    return truncatedBody;
  });
  await assert.rejects(truncated.get("k"), {
    message: "KV_INTERNAL_PROTOCOL_ERROR",
  });
  assert.equal(truncatedBody.body.locked, false);
  const extraValue = hanging(Buffer.concat([header(1), Buffer.from([9, 8])]));
  const extraKv = transport(extraValue.fetch);
  await assert.rejects(extraKv.get("k"), {
    message: "KV_INTERNAL_PROTOCOL_ERROR",
  });
  assert.equal(extraValue.cancelled(), true);
  assert.equal(extraValue.locked(), false);
});

test("KV bulk decoder rejects non-canonical entries and cancels the backend reader", async () => {
  const countPrefix = (count) => {
    const prefix = Buffer.alloc(6);
    prefix.write("KVB1");
    prefix.writeUInt16BE(count, 4);
    return prefix;
  };
  const cases = [
    [
      "found marker 2",
      bulkFields({ found: 2, metadataLength: 0xffffffff, valueLength: 1 }),
    ],
    [
      "missing with metadata length",
      bulkFields({ found: 0, metadataLength: 4, valueLength: 0xffffffff }),
    ],
    [
      "missing with expiration",
      bulkFields({
        found: 0,
        expiration: 9n,
        metadataLength: 0xffffffff,
        valueLength: 0xffffffff,
      }),
    ],
    [
      "missing with value length",
      bulkFields({ found: 0, metadataLength: 0xffffffff, valueLength: 3 }),
    ],
    [
      "unsafe metadata length",
      bulkFields({ found: 1, metadataLength: 2048, valueLength: 1 }),
    ],
    ["count mismatch", countPrefix(2)],
    [
      "trailing bytes",
      Buffer.concat([countPrefix(1), bulkEntry(null), Buffer.from([7])]),
    ],
  ];
  for (const [label, bytes] of cases) {
    const hung = hanging(bytes);
    const kv = transport(hung.fetch);
    await assert.rejects(
      kv.get(["k"]),
      { message: "KV_INTERNAL_PROTOCOL_ERROR" },
      label,
    );
    assert.equal(hung.cancelled(), true, `${label} must cancel`);
    assert.equal(hung.locked(), false, `${label} must release the body lock`);
  }
  let truncatedBody;
  const truncated = transport(async () => {
    truncatedBody = new Response(Buffer.from("KVB1"), {
      headers: { "content-type": contentType },
    });
    return truncatedBody;
  });
  await assert.rejects(truncated.get(["k"]), {
    message: "KV_INTERNAL_PROTOCOL_ERROR",
  });
  assert.equal(truncatedBody.body.locked, false);
});

test("KV denies undeclared permissions and exposes no echo extension", async () => {
  const kv = transport(
    async () => {
      throw new Error("must not reach backend");
    },
    { read: false, write: false },
  );
  await assert.rejects(kv.get("key"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.put("key", "value"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.delete("key"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.list(), /BINDING_PERMISSION_DENIED/);
  const denied = await kv.fetch(
    new Request("https://fake-host/key?urlencoded=true"),
  );
  assert.equal(denied.status, 403);
  assert.equal(denied.statusText, "BINDING_PERMISSION_DENIED");
  assert.equal(kv.echoStream, undefined);
  const failed = transport(
    async () =>
      new Response("private details", {
        status: 503,
        headers: { "x-open-compute-error-code": "KV_RESULT_UNKNOWN" },
      }),
  );
  await assert.rejects(failed.put("key", "value"), {
    message: "KV_RESULT_UNKNOWN",
  });
});

test("native KV preserves sanitized resource authority errors", async () => {
  const kv = transport(
    async () =>
      new Response("private resource details", {
        status: 503,
        headers: { "x-open-compute-error-code": "RESOURCE_UNAVAILABLE" },
      }),
  );
  const response = await kv.fetch(
    new Request("https://fake-host/key?urlencoded=true"),
  );
  assert.equal(response.status, 500);
  assert.equal(response.statusText, "RESOURCE_UNAVAILABLE");
  assert.equal(await response.text(), "");
});

test("native KV denied PUT drains the upload and leaves storage untouched", async () => {
  const kv = transport(
    async () => {
      throw new Error("denied PUT must not reach storage");
    },
    { read: true, write: false },
  );
  const direct = new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array([1, 2, 3]));
      controller.close();
    },
  });
  await assert.rejects(kv.put("key", direct), /BINDING_PERMISSION_DENIED/);
  assert.equal(direct.locked, false);
  const request = new Request("https://fake-host/key?urlencoded=true", {
    method: "PUT",
    body: "denied",
  });
  const response = await kv.fetch(request);
  assert.equal(response.status, 403);
  assert.equal(response.statusText, "BINDING_PERMISSION_DENIED");
  assert.equal(request.bodyUsed, true);
  assert.equal(request.body.locked, false);
  assert.equal(await response.text(), "");

  let cancelled = false;
  const oversized = new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array(25 * 1024 * 1024 + 1));
    },
    cancel() {
      cancelled = true;
    },
  });
  const overflow = new Request("https://fake-host/key?urlencoded=true", {
    method: "PUT",
    body: oversized,
    duplex: "half",
  });
  assert.equal((await kv.fetch(overflow)).status, 403);
  assert.equal(cancelled, true);
  assert.equal(overflow.body.locked, false);
});

test("native KV translates an invalid authority cursor to the native error", async () => {
  const kv = transport(
    async () =>
      new Response("private cursor details", {
        status: 400,
        headers: { "x-open-compute-error-code": "KV_CURSOR_INVALID" },
      }),
  );
  const response = await kv.fetch(
    new Request("https://fake-host/?cursor=tampered"),
  );
  assert.equal(response.status, 400);
  assert.equal(response.statusText, "Invalid cursor");
  assert.equal(await response.text(), "");
});

test("native KV rejects invalid wire inputs with public protocol errors", async () => {
  const kv = transport(async () => {
    throw new Error("invalid input must not reach the authority");
  });
  const cases = [
    ["/bulk/get", { keys: [] }, 400, "You must request a minimum of 1 key"],
    [
      "/bulk/get",
      { keys: Array(101).fill("k") },
      400,
      "You can request a maximum of 100 keys",
    ],
    ["/bulk/get", { keys: ["."] }, 400, "Key name . is not legal"],
    [
      "/bulk/get",
      { keys: ["x".repeat(513)] },
      414,
      "Encoded length of 513 is too long",
    ],
    [
      "/bulk/get",
      { keys: ["key"], type: "stream" },
      400,
      '"stream" is not a valid type. Use "json" or "text"',
    ],
    [
      "/bulk/get",
      { keys: ["key"], cacheTtl: 29 },
      400,
      "Invalid cache_ttl of 29. Cache TTL must be at least 30.",
    ],
    [
      "/key?urlencoded=true&cache_ttl=29",
      undefined,
      400,
      "Invalid cache_ttl of 29. Cache TTL must be at least 30.",
    ],
    [
      "/?key_count_limit=1001",
      undefined,
      400,
      "Invalid key_count_limit of 1001. Please specify integer less than 1000.",
    ],
    [
      "/%ED%A0%80?urlencoded=true",
      undefined,
      400,
      "Could not URL-decode key name",
    ],
  ];
  for (const [path, input, status, message] of cases) {
    const response = await kv.fetch(
      new Request(
        `https://fake-host${path}`,
        input === undefined
          ? {}
          : { method: "POST", body: JSON.stringify(input) },
      ),
    );
    assert.equal(response.status, status, path);
    assert.equal(response.statusText, message, path);
    assert.equal(await response.text(), "", path);
  }
  const request = new Request(
    "https://fake-host/key?urlencoded=true&expiration_ttl=59",
    {
      method: "PUT",
      body: "value",
    },
  );
  const response = await kv.fetch(request);
  assert.equal(response.status, 400);
  assert.equal(
    response.statusText,
    "Invalid expiration_ttl of 59. Expiration TTL must be at least 60.",
  );
  assert.equal(request.bodyUsed, true);
  assert.equal(await response.text(), "");
});

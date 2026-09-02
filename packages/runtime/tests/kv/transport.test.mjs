import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const base = moduleUrl("export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }");
const host = moduleUrl("export const bindingError = code => new Error(code); export const currentStartupGeneration = () => 'generation';");
const { KVNamespace } = await importRuntime("kv/transport.ts", {
  "cloudflare:workers": base,
  "../loader/host.js": host,
});
const contentType = "application/vnd.open-compute.kv.v1+frame";
const props = { bindingId: "binding", versionId: "version", descriptorSha256: "a".repeat(64),
  resourceSpecGeneration: 1, permissions: { read: true, write: true } };
const transport = (fetch, permissions = props.permissions) => new KVNamespace(
  { props: { ...props, permissions } }, { BINDING_BACKEND: { fetch }, BINDING_BACKEND_TOKEN: "token" },
);

function header(valueLength, metadata = null) {
  const encoded = metadata === null ? null : Buffer.from(JSON.stringify(metadata));
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
  return new Response(Buffer.concat([header(bytes?.length ?? null, metadata), bytes ?? Buffer.alloc(0)]), {
    headers: { "content-type": contentType },
  });
}

function bulkEntry(value, metadata = null) {
  const encoded = metadata === null ? null : Buffer.from(JSON.stringify(metadata));
  const valueBytes = value === null ? null : Buffer.from(value);
  const bytes = Buffer.alloc(17 + (encoded?.length ?? 0) + (valueBytes?.length ?? 0));
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
    chunks.push(entry === null ? bulkEntry(null) : bulkEntry(entry.value, entry.metadata ?? null));
  }
  return new Response(Buffer.concat(chunks), { headers: { "content-type": contentType } });
}

test("KV uses one frame protocol for default text, binary, JSON and metadata reads", async () => {
  const calls = [];
  const kv = transport(async (url, options) => {
    assert.equal(options.headers["content-type"], contentType);
    assert.equal(options.headers["x-open-compute-startup-generation"], "generation");
    calls.push({ operation: url.split("/").at(-1), request: JSON.parse(options.body) });
    return result('{"ok":true}', { owner: "app" });
  });
  assert.equal(await kv.get("key"), '{"ok":true}');
  assert.deepEqual(await kv.get("key", "json"), { ok: true });
  assert.deepEqual(Buffer.from(await kv.get("key", "arrayBuffer")), Buffer.from('{"ok":true}'));
  assert.deepEqual(await kv.get("key", { type: "text" }), '{"ok":true}');
  assert.deepEqual(await kv.getWithMetadata("key", { type: "json", cacheTtl: 30 }), {
    value: { ok: true }, metadata: { owner: "app" }, cacheStatus: null,
  });
  assert.deepEqual(calls.at(-1), { operation: "get-with-metadata", request: { keys: ["key"], cacheTtl: 30 } });
  assert.equal(await transport(async () => result(null)).get("missing"), null);
  assert.deepEqual(await transport(async () => result(null)).getWithMetadata("missing"), {
    value: null, metadata: null, cacheStatus: null,
  });
  assert.deepEqual(new Uint8Array(await transport(async () => result([0, 255])).get("binary", "arrayBuffer")), new Uint8Array([0, 255]));
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
  assert.deepEqual([...await kv.get(["one", "missing", "one"], "json")], [["one", { ok: true }], ["missing", null]]);
  assert.equal(response.body.locked, false);
  assert.deepEqual([...await kv.getWithMetadata(["one", "missing", "one"], { type: "json" })], [
    ["one", { value: { ok: true }, metadata: { a: 1 } }],
    ["missing", null],
  ]);
  assert.equal(response.body.locked, false);
});

test("KV streams propagate cancellation to the backend without buffering the value", async () => {
  let pulled = 0;
  let cancelled = false;
  const kv = transport(async () => new Response(new ReadableStream({
    start(controller) { controller.enqueue(header(100_000)); },
    pull(controller) { pulled++; controller.enqueue(new Uint8Array([1])); },
    cancel() { cancelled = true; },
  }), { headers: { "content-type": contentType } }));
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
    calls.push({ operation: url.split("/").at(-1), bytes: Buffer.from(await new Response(options.body).arrayBuffer()) });
    return new Response(null, { status: 204 });
  });
  for (const value of [new Uint8Array([0, 255]), new ReadableStream({ start(controller) {
    controller.enqueue(new Uint8Array([0])); controller.enqueue(new Uint8Array([255])); controller.close();
  } })]) {
    await kv.put("key", value, { metadata: { ok: true }, expirationTtl: 60 });
    const { operation, bytes } = calls.at(-1);
    assert.equal(operation, "put");
    const length = bytes.readUInt32BE(0);
    assert.deepEqual(JSON.parse(bytes.subarray(4, 4 + length)), {
      key: "key", metadata: { ok: true }, metadataPresent: true, expirationTtl: 60,
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
  await assert.rejects(kv.put("gone", detached), /KV value must be a string, buffer, view, or ReadableStream/);
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
  const kv = transport(async () => { throw new Error("must not reach backend"); });
  await assert.rejects(kv.get(""), { name: "TypeError", message: "KV_KEY_INVALID" });
  await assert.rejects(kv.get("."), { name: "TypeError", message: "KV_KEY_INVALID" });
  await assert.rejects(kv.get(".."), { name: "TypeError", message: "KV_KEY_INVALID" });
  await assert.rejects(kv.get("\uD800"), { name: "TypeError", message: "KV_KEY_INVALID" });
  await assert.rejects(kv.get([]), { name: "TypeError", message: "KV_TOO_MANY_KEYS" });
  await assert.rejects(kv.get(Array.from({ length: 101 }, (_, i) => `k${i}`)), { name: "TypeError", message: "KV_TOO_MANY_KEYS" });
  await assert.rejects(kv.get("k", "banana"), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
  await assert.rejects(kv.get(["k"], "arrayBuffer"), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
  await assert.rejects(kv.get("k", { cacheTtl: 29 }), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
  await assert.rejects(kv.put("k", "v", { expiration: 10, expirationTtl: 60 }), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
  await assert.rejects(kv.put("k", "v", { expirationTtl: 59 }), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
  await assert.rejects(kv.put("k", {}), /KV value must be a string, buffer, view, or ReadableStream/);
  await assert.rejects(kv.list({ prefix: 1 }), { name: "TypeError", message: "KV_KEY_INVALID" });
  await assert.rejects(kv.list({ limit: 0 }), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
  await assert.rejects(kv.list({ extra: true }), { name: "TypeError", message: "KV_INVALID_OPTIONS" });
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
    return new Response(Buffer.concat([bytes, Buffer.from("ab")]), { headers: { "content-type": contentType } });
  });
  await assert.rejects(corrupt.getWithMetadata("k"), { message: "KV_INTERNAL_PROTOCOL_ERROR" });
});

function hanging(bytes) {
  let cancelled = false;
  let response;
  return {
    cancelled: () => cancelled,
    locked: () => response.body.locked,
    fetch: async () => {
      response = new Response(new ReadableStream({
        start(controller) { controller.enqueue(Buffer.from(bytes)); },
        cancel() { cancelled = true; },
      }), { headers: { "content-type": contentType } });
      return response;
    },
  };
}

function singleFields({ found = 1, expiration = -1n, metadataLength, valueLength }) {
  const bytes = Buffer.alloc(21);
  bytes.write("KVS1");
  bytes[4] = found;
  bytes.writeBigInt64BE(expiration, 5);
  bytes.writeUInt32BE(metadataLength, 13);
  bytes.writeUInt32BE(valueLength, 17);
  return bytes;
}

function bulkFields({ found = 1, expiration = -1n, metadataLength, valueLength }) {
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
    ["found marker 2", singleFields({ found: 2, metadataLength: 0xffffffff, valueLength: 1 })],
    ["found marker 255", singleFields({ found: 255, metadataLength: 0xffffffff, valueLength: 1 })],
    ["missing with metadata length", singleFields({ found: 0, metadataLength: 2, valueLength: 0xffffffff })],
    ["missing with expiration", singleFields({ found: 0, expiration: 1n, metadataLength: 0xffffffff, valueLength: 0xffffffff })],
    ["missing with value length", singleFields({ found: 0, metadataLength: 0xffffffff, valueLength: 0 })],
    ["unsafe metadata length", singleFields({ found: 1, metadataLength: 1025, valueLength: 1 })],
    ["unsafe value length", singleFields({ found: 1, metadataLength: 0xffffffff, valueLength: 25 * 1024 * 1024 + 1 })],
    ["trailing bytes after missing", Buffer.concat([header(null), Buffer.from([1])])],
  ];
  for (const [label, bytes] of cases) {
    const hung = hanging(bytes);
    const kv = transport(hung.fetch);
    await assert.rejects(kv.get("k"), { message: "KV_INTERNAL_PROTOCOL_ERROR" }, label);
    assert.equal(hung.cancelled(), true, `${label} must cancel`);
    assert.equal(hung.locked(), false, `${label} must release the body lock`);
  }
  let truncatedBody;
  const truncated = transport(async () => {
    truncatedBody = new Response(Buffer.from("KVS1"), { headers: { "content-type": contentType } });
    return truncatedBody;
  });
  await assert.rejects(truncated.get("k"), { message: "KV_INTERNAL_PROTOCOL_ERROR" });
  assert.equal(truncatedBody.body.locked, false);
  const extraValue = hanging(Buffer.concat([header(1), Buffer.from([9, 8])]));
  const extraKv = transport(extraValue.fetch);
  await assert.rejects(extraKv.get("k"), { message: "KV_INTERNAL_PROTOCOL_ERROR" });
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
    ["found marker 2", bulkFields({ found: 2, metadataLength: 0xffffffff, valueLength: 1 })],
    ["missing with metadata length", bulkFields({ found: 0, metadataLength: 4, valueLength: 0xffffffff })],
    ["missing with expiration", bulkFields({ found: 0, expiration: 9n, metadataLength: 0xffffffff, valueLength: 0xffffffff })],
    ["missing with value length", bulkFields({ found: 0, metadataLength: 0xffffffff, valueLength: 3 })],
    ["unsafe metadata length", bulkFields({ found: 1, metadataLength: 2048, valueLength: 1 })],
    ["count mismatch", countPrefix(2)],
    ["trailing bytes", Buffer.concat([countPrefix(1), bulkEntry(null), Buffer.from([7])])],
  ];
  for (const [label, bytes] of cases) {
    const hung = hanging(bytes);
    const kv = transport(hung.fetch);
    await assert.rejects(kv.get(["k"]), { message: "KV_INTERNAL_PROTOCOL_ERROR" }, label);
    assert.equal(hung.cancelled(), true, `${label} must cancel`);
    assert.equal(hung.locked(), false, `${label} must release the body lock`);
  }
  let truncatedBody;
  const truncated = transport(async () => {
    truncatedBody = new Response(Buffer.from("KVB1"), { headers: { "content-type": contentType } });
    return truncatedBody;
  });
  await assert.rejects(truncated.get(["k"]), { message: "KV_INTERNAL_PROTOCOL_ERROR" });
  assert.equal(truncatedBody.body.locked, false);
});

test("KV denies undeclared permissions and exposes no echo extension", async () => {
  const kv = transport(async () => { throw new Error("must not reach backend"); }, { read: false, write: false });
  await assert.rejects(kv.get("key"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.put("key", "value"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.delete("key"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.list(), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.fetch(), /BINDING_PERMISSION_DENIED/);
  assert.equal(kv.echoStream, undefined);
  const failed = transport(async () => new Response("private details", {
    status: 503, headers: { "x-open-compute-error-code": "KV_RESULT_UNKNOWN" },
  }));
  await assert.rejects(failed.put("key", "value"), { message: "KV_RESULT_UNKNOWN" });
});

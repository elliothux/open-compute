import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const facade = moduleUrl(await compileRuntime("kv/facade.ts"));
const { KVNamespace } = await import(facade);

function raw(captures) {
  return {
    async get() { return null; },
    async getWithMetadata() { return { value: null, metadata: null, cacheStatus: null }; },
    async put(key, value, options) {
      await Promise.resolve();
      captures.push({ key, value: Array.from(value), options });
    },
    async delete() {},
    async list() { return { keys: [], list_complete: true, cacheStatus: null }; },
  };
}

test("KV facade snapshots resizable and shared views before RPC serialization", async () => {
  const captures = [];
  const kv = new KVNamespace(raw(captures));
  const resizable = new ArrayBuffer(4, { maxByteLength: 16 });
  new Uint8Array(resizable).set([9, 8, 7, 6]);
  const pendingResizable = kv.put("rab", resizable, { expirationTtl: 60 });
  resizable.resize(0);
  await pendingResizable;

  if (typeof SharedArrayBuffer === "function") {
    const shared = new SharedArrayBuffer(3);
    const view = new Uint8Array(shared);
    view.set([1, 2, 3]);
    const pendingShared = kv.put("sab", view);
    view.set([9, 9, 9]);
    await pendingShared;
  }

  assert.deepEqual(captures, [
    { key: "rab", value: [9, 8, 7, 6], options: { expirationTtl: 60 } },
    ...typeof SharedArrayBuffer === "function"
      ? [{ key: "sab", value: [1, 2, 3], options: undefined }]
      : [],
  ]);
});

test("KV facade rejects a detached BufferSource before invoking the transport", async () => {
  const captures = [];
  const kv = new KVNamespace(raw(captures));
  const detached = new ArrayBuffer(4);
  if (typeof detached.transfer === "function") detached.transfer();
  else structuredClone(detached, { transfer: [detached] });
  await assert.rejects(
    kv.put("detached", detached),
    /KV put\(\) accepts only strings, ArrayBuffers, ArrayBufferViews, and ReadableStreams as values\./,
  );
  assert.deepEqual(captures, []);
});

test("KV facade applies Cloudflare key, option and metadata conversions before transport", async () => {
  const calls = [];
  const kv = new KVNamespace({
    async get(key, options) { calls.push(["get", key, options]); return null; },
    async getWithMetadata(key, options) { calls.push(["getWithMetadata", key, options]); return null; },
    async put(key, value, options) { calls.push(["put", key, value, options]); },
    async delete(key) { calls.push(["delete", key]); },
    async list(options) { calls.push(["list", options]); return { keys: [], list_complete: true, cacheStatus: null }; },
  });

  await kv.get(1, { type: "text", cacheTtl: "30", ignored: true });
  await kv.get(["a", 2], { type: "json" });
  await kv.get(["\ud800"]);
  await kv.getWithMetadata(null, null);
  await kv.put("both", "value", {
    expiration: 1,
    expirationTtl: 60,
    metadata: { finite: 1, infinite: Infinity },
    ignored: true,
  });
  await kv.delete(3);
  await kv.list({ prefix: 1, cursor: null, limit: 1.9, ignored: true });
  await kv.list({ prefix: "\ud800" });
  await kv.list({ limit: 0 });

  assert.deepEqual(calls, [
    ["get", "1", { type: "text", cacheTtl: 30 }],
    ["get", ["a", "2"], { type: "json" }],
    ["get", ["\ufffd\ufffd\ufffd"], { type: "text" }],
    ["getWithMetadata", "null", { type: "text" }],
    ["put", "both", "value", { expirationTtl: 60, metadata: { finite: 1, infinite: null } }],
    ["delete", "3"],
    ["list", { prefix: "1", limit: 1 }],
    ["list", { prefix: "\ufffd\ufffd\ufffd" }],
    ["list", undefined],
  ]);
});

test("KV facade matches Cloudflare rejection type, text, and Promise timing", async () => {
  const kv = new KVNamespace(raw([]));
  const cases = [
    [() => kv.get(""), TypeError, "Key name cannot be empty."],
    [() => kv.get("."), TypeError, '"." is not allowed as a key name.'],
    [() => kv.delete(".."), TypeError, '".." is not allowed as a key name.'],
    [() => kv.get("x".repeat(513)), Error,
      "KV GET failed: 414 UTF-8 encoded length of 513 exceeds key length limit of 512."],
    [() => kv.get("\ud800"), Error, "KV GET failed: 400 Could not URL-decode key name"],
    [() => kv.get([]), Error, "KV GET_BULK failed: 400 You must request a minimum of 1 key"],
    [() => kv.get([""]), Error, "KV GET_BULK failed: 400 Key name  is not legal"],
    [() => kv.get(["."]), Error, "KV GET_BULK failed: 400 Key name . is not legal"],
    [() => kv.get(["x".repeat(513)]), Error,
      "KV GET_BULK failed: 414 Encoded length of 513 is too long"],
    [() => kv.getWithMetadata([".."]), Error,
      "KV GET_BULK failed: 400 Key name .. is not legal"],
    [() => kv.get(Array.from({ length: 101 }, (_, index) => `k${index}`)), Error,
      "KV GET_BULK failed: 400 You can request a maximum of 100 keys"],
    [() => kv.get("key", "banana"), TypeError,
      'Unknown response type. Possible types are "text", "arrayBuffer", "json", and "stream".'],
    [() => kv.get(["key"], "stream"), Error,
      'KV GET_BULK failed: 400 "stream" is not a valid type. Use "json" or "text"'],
    [() => kv.get("key", { cacheTtl: 29 }), Error,
      "KV GET failed: 400 Invalid cache_ttl of 29. Cache TTL must be at least 30."],
    [() => kv.put("key", "value", { expirationTtl: 59 }), Error,
      "KV PUT failed: 400 Invalid expiration_ttl of 59. Expiration TTL must be at least 60."],
    [() => kv.put("key", {}), TypeError,
      "KV put() accepts only strings, ArrayBuffers, ArrayBufferViews, and ReadableStreams as values."],
    [() => kv.list({ limit: 1001 }), Error,
      "KV GET failed: 400 Invalid key_count_limit of 1001. Please specify integer less than 1000."],
  ];
  for (const [call, constructor, message] of cases) {
    let pending;
    assert.doesNotThrow(() => { pending = call(); });
    await assert.rejects(pending, { name: constructor.name, message });
  }
});

test("KV facade maps private cursor and streaming limit codes to public errors", async () => {
  const kv = new KVNamespace({
    ...raw([]),
    async put() { throw { stableCode: "KV_VALUE_TOO_LARGE" }; },
    async list() { throw Object.assign(Object.create(null), { message: "KV_CURSOR_INVALID" }); },
  });
  await assert.rejects(kv.put("key", new ReadableStream()), {
    name: "Error",
    message: "KV PUT failed: 413 Value length of 26214401 exceeds limit of 26214400.",
  });
  await assert.rejects(kv.list({ cursor: "invalid" }), {
    name: "Error",
    message: "KV GET failed: 400 Invalid cursor",
  });
});

import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const workerBase = moduleUrl("export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }");
const compiled = await mkdtemp(join(tmpdir(), "oc-r2-runtime-"));
await writeFile(join(compiled, "transport.mjs"), await compileRuntime("r2/transport.ts", { "cloudflare:workers": workerBase }));
await writeFile(join(compiled, "facade.mjs"), await compileRuntime("r2/facade.ts"));
const { makeR2TransportBase } = await import(pathToFileURL(join(compiled, "transport.mjs")).href);
const { R2Bucket } = await import(pathToFileURL(join(compiled, "facade.mjs")).href);
const props = { bindingId: "binding", deploymentId: "deployment", descriptorSha256: "a".repeat(64), resourceSpecGeneration: 1, permissions: { read: true, write: true } };
const meta = { key: "asset", version: "00000000-0000-7000-8000-000000000001", size: 3, etag: "etag", httpEtag: '"etag"', uploaded: 1,
  httpMetadata: { contentType: "text/plain", cacheExpiry: null }, customMetadata: {}, checksums: { md5: "5d41402abc4b2a76b9719d911017c592" }, storageClass: "Standard" };
const Transport = makeR2TransportBase(code => new Error(code), () => "generation", "private-token");
const transport = fetch => new Transport({ props }, { BINDING_BACKEND: { fetch }, BINDING_BACKEND_TOKEN: "token" });

function frame(metadata, body) {
  const header = new TextEncoder().encode(JSON.stringify({ meta: metadata, hasBody: body !== undefined }));
  const prefix = new Uint8Array(4);
  new DataView(prefix.buffer).setUint32(0, header.length);
  return new Response(new ReadableStream({ start(controller) {
    for (const part of [prefix, header, ...(body === undefined ? [] : [new TextEncoder().encode(body)])]) controller.enqueue(part);
    controller.close();
  } }));
}

test("compiled R2 transport and facade preserve metadata and one-shot streamed bodies", async () => {
  const bucket = new R2Bucket(transport(async () => frame(meta, "abc")));
  const object = await bucket.get("asset");
  assert.equal(object.uploaded.getTime(), 1);
  assert.equal(object.httpMetadata.contentType, "text/plain");
  assert.equal(await object.text(), "abc");
  assert.equal(object.bodyUsed, true);
  await assert.rejects(
    object.bytes(),
    /Body has already been used\. It can only be used once\. Use tee\(\) first if you need to read it twice\./,
  );
  const head = await new R2Bucket(transport(async () => Response.json(meta))).head("asset");
  assert.equal(head.httpEtag, '"etag"');
  const ranged = await new R2Bucket(transport(async () => Response.json({
    ...meta,
    range: { offset: 1, length: 2, suffix: null },
  }))).head("asset");
  assert.deepEqual(ranged.range, { offset: 1, length: 2 });
  const metadataUnknown = await new R2Bucket(transport(async () => Response.json({
    ...meta,
    httpMetadata: null,
  }))).head("asset");
  assert.throws(
    () => metadataUnknown.writeHttpMetadata(new Headers()),
    /HTTP metadata unknown for key `asset`\. Did you forget to add 'httpMetadata' to `include` when listing\?/,
  );
  const list = await transport(async () => Response.json({ objects: [meta], truncated: false, delimitedPrefixes: [] })).list({ prefix: "", limit: 10, include: [] });
  assert.equal(list.objects[0].key, "asset");
});

test("compiled R2 transport rejects malformed metadata and incomplete frames", async () => {
  for (const invalid of [null, { ...meta, size: "3" }, { ...meta, customMetadata: { x: 1 } },
    { ...meta, range: { offset: -1 } }, { ...meta, httpMetadata: { cacheExpiry: "tomorrow" } }]) {
    await assert.rejects(transport(async () => frame(invalid, "abc")).get("asset", {}), /BINDING_PROTOCOL_ERROR/);
    await assert.rejects(transport(async () => Response.json(invalid)).head("asset"), /BINDING_PROTOCOL_ERROR/);
  }
  await assert.rejects(transport(async () => new Response(new Uint8Array([0, 0]))).get("asset", {}), /BINDING_PROTOCOL_ERROR/);
});

test("R2 requires a complete transport and forwards cancellation to its stream", async () => {
  assert.throws(() => new R2Bucket({ async get() {} }), /R2_INTERNAL_PROTOCOL_ERROR/);
  let cancelled = false;
  const unexpected = () => { throw new Error("unexpected transport call"); };
  const bucket = new R2Bucket({
    head: unexpected, put: unexpected, delete: unexpected, list: unexpected,
    createMultipartUpload: unexpected, uploadPart: unexpected, completeMultipartUpload: unexpected, abortMultipartUpload: unexpected,
    async get() {
      return { meta, body: new ReadableStream({
        start(controller) { controller.enqueue(new Uint8Array([1, 2, 3])); },
        cancel() { cancelled = true; },
      }) };
    },
  });
  const reader = (await bucket.get("asset")).body.getReader();
  assert.equal((await reader.read()).value.byteLength, 3);
  await reader.cancel("consumer stopped");
  assert.equal(cancelled, true);
});

test("compiled R2 facade exposes checksums.toJSON, list cursor union, and local multipart resume", async () => {
  const bucket = new R2Bucket(transport(async () => frame(meta, "abc")));
  const object = await bucket.get("asset");
  assert.deepEqual(object.checksums.toJSON(), { md5: "5d41402abc4b2a76b9719d911017c592" });
  assert.equal(object.version, meta.version);
  assert.equal(object.storageClass, "Standard");
  const truncated = await new R2Bucket(transport(async () => Response.json({
    objects: [meta], truncated: true, cursor: "next", delimitedPrefixes: ["p/"],
  }))).list({ limit: 1 });
  assert.equal(truncated.truncated, true);
  assert.equal(truncated.cursor, "next");
  const complete = await new R2Bucket(transport(async () => Response.json({
    objects: [meta], truncated: false, delimitedPrefixes: [],
  }))).list({ prefix: "", limit: 10, include: [], startAfter: "a" });
  assert.equal(complete.truncated, false);
  assert.equal("cursor" in complete, true);
  assert.equal(complete.cursor, undefined);
  const created = await new R2Bucket({
    async head() { return null; },
    async get() { return null; },
    async put() { return meta; },
    async delete() {},
    async list() { return { objects: [], truncated: false, delimitedPrefixes: [] }; },
    async createMultipartUpload(key) { return { key, uploadId: "upload" }; },
    async uploadPart(_key, _uploadId, partNumber) { return { partNumber, etag: "part" }; },
    async completeMultipartUpload() { return meta; },
    async abortMultipartUpload() {},
  }).createMultipartUpload("mpu");
  const resumed = new R2Bucket({
    async head() { return null; },
    async get() { return null; },
    async put() { return meta; },
    async delete() {},
    async list() { return { objects: [], truncated: false, delimitedPrefixes: [] }; },
    async createMultipartUpload() { throw new Error("unexpected"); },
    async uploadPart(key, uploadId, partNumber) {
      assert.equal(key, "mpu");
      assert.equal(uploadId, "upload");
      return { partNumber, etag: "part" };
    },
    async completeMultipartUpload() { return meta; },
    async abortMultipartUpload() {},
  }).resumeMultipartUpload(created.key, created.uploadId);
  assert.equal(resumed.uploadId, "upload");
  const uploaded = await resumed.uploadPart(1, "body");
  assert.equal(uploaded.etag, "part");
  const completed = await resumed.complete([uploaded]);
  assert.equal(completed.httpMetadata, undefined);
  assert.equal(completed.customMetadata, undefined);
  assert.equal(completed.storageClass, "Standard");
  await assert.rejects(
    new R2Bucket(transport(async () => frame(meta))).put("k", "v", { md5: "00".repeat(16), sha1: "00".repeat(20) }),
    /You cannot specify multiple hashing algorithms\./,
  );
  await assert.rejects(
    new R2Bucket(transport(async () => frame(meta))).put("k", "v", { onlyIf: { etagMatches: "\"quoted\"" } }),
    /Conditional ETag should not be wrapped in quotes \("quoted"\)\./,
  );
});

test("Headers conditions retain HTTP precedence and object conditions remain conjunctive", async () => {
  const calls = [];
  const unexpected = () => { throw new Error("unexpected transport call"); };
  const raw = {
    head: unexpected,
    async get(_key, options) { calls.push(options.onlyIf); return null; },
    async put(_key, _body, options) { calls.push(options.onlyIf); return meta; },
    delete: unexpected,
    list: unexpected,
    createMultipartUpload: unexpected,
    uploadPart: unexpected,
    completeMultipartUpload: unexpected,
    abortMultipartUpload: unexpected,
  };
  const bucket = new R2Bucket(raw);
  await bucket.get("asset", { onlyIf: new Headers({
    "if-match": 'W/"weak", "strong"',
    "if-unmodified-since": new Date(0).toUTCString(),
  }) });
  await bucket.put("asset", "body", { onlyIf: {
    etagDoesNotMatch: "other",
    uploadedAfter: new Date(0),
    secondsGranularity: true,
  } });
  assert.deepEqual(calls[0], {
    etagMatches: [
      { kind: "weak", value: "weak" },
      { kind: "strong", value: "strong" },
    ],
    etagDoesNotMatch: [],
    secondsGranularity: true,
    httpHeaders: true,
    uploadedBefore: 0,
  });
  assert.deepEqual(calls[1], {
    etagMatches: [],
    etagDoesNotMatch: [{ kind: "strong", value: "other" }],
    secondsGranularity: true,
    httpHeaders: false,
    uploadedAfter: 0,
  });
});

test("R2 facade matches pinned workerd coercion and validation behavior", async () => {
  const calls = [];
  const raw = {
    async head(key) { calls.push(["head", key]); return null; },
    async get(key, options) { calls.push(["get", key, options]); return null; },
    async put(key, _body, options) { calls.push(["put", key, options]); return meta; },
    async delete(keys) { calls.push(["delete", keys]); },
    async list(options) {
      calls.push(["list", options]);
      return options.limit === 0
        ? { objects: [], truncated: true, cursor: "zero", delimitedPrefixes: [] }
        : { objects: [], truncated: false, delimitedPrefixes: [] };
    },
    async createMultipartUpload(key, options) {
      calls.push(["createMultipartUpload", key, options]);
      return { key, uploadId: "upload" };
    },
    async uploadPart(key, uploadId, partNumber, _body, ssecKey) {
      calls.push(["uploadPart", key, uploadId, partNumber, ssecKey]);
      return { partNumber, etag: "part" };
    },
    async completeMultipartUpload(key, uploadId, parts) {
      calls.push(["complete", key, uploadId, parts]);
      return { ...meta, key };
    },
    async abortMultipartUpload(key, uploadId) { calls.push(["abort", key, uploadId]); },
  };
  const bucket = new R2Bucket(raw);

  await bucket.get("seed", { unknown: true });
  await bucket.put("unknown", "value", { unknown: true });
  await bucket.list({ unknown: true });
  await bucket.get(1);
  await bucket.put("\ud800", "value");
  assert.deepEqual(calls.slice(0, 5).map(call => call.slice(0, 2)), [
    ["get", "seed"], ["put", "unknown"], ["list", { prefix: "", limit: 1000, include: [] }],
    ["get", "1"], ["put", "���"],
  ]);
  await assert.rejects(bucket.get(Symbol("key")), {
    name: "TypeError", message: "Cannot convert a Symbol value to a string",
  });

  for (const [range, name, message] of [
    [{ offset: -1 }, "RangeError", "Invalid range. Starting offset (-1) must be greater than or equal to 0."],
    [{ offset: 0.5 }, "RangeError", "Invalid range. Starting offset (0.5) must be an integer, not floating point."],
    [{ length: -1 }, "RangeError", "Invalid range. Length (-1) must be greater than or equal to 0."],
    [{ suffix: -1 }, "RangeError", "Invalid suffix. Suffix (-1) must be greater than or equal to 0."],
    [{ suffix: 1, offset: 0 }, "TypeError", "Suffix is incompatible with offset."],
    [{ suffix: 1, length: 1 }, "TypeError", "Suffix is incompatible with length."],
  ]) {
    await assert.rejects(bucket.get("seed", { range }), { name, message });
  }
  await bucket.get("seed", { range: new Headers({ range: "bytes=0-1,3-4" }) });
  assert.deepEqual(calls.at(-1), ["get", "seed", {}]);

  await assert.rejects(bucket.get("seed", { onlyIf: { etagMatches: '"quoted"' } }), {
    name: "TypeError", message: 'Conditional ETag should not be wrapped in quotes ("quoted").',
  });
  await assert.rejects(bucket.get("seed", { onlyIf: new Headers({ "if-match": "bad" }) }), {
    name: "Error", message: "Invalid ETag in if-match header",
  });
  for (const [options, message] of [
    [{ md5: new Uint8Array(15) }, "MD5 is 16 bytes, not 15"],
    [{ md5: "00" }, "MD5 is 32 hex characters, not 2"],
    [{ md5: "z".repeat(32) }, "Provided MD5 wasn't a valid hex string"],
    [{ md5: "00".repeat(16), sha1: "00".repeat(20) }, "You cannot specify multiple hashing algorithms."],
  ]) {
    await assert.rejects(bucket.put("seed", "value", options), { name: "TypeError", message });
  }
  await assert.rejects(bucket.get("seed", { ssecKey: "Z".repeat(64) }), {
    name: "Error", message: "SSE-C Key has invalid format",
  });
  await assert.rejects(bucket.get("seed", { ssecKey: "00" }), {
    name: "Error", message: "SSE-C Key must be 32 bytes in length",
  });
  await assert.rejects(bucket.put("seed", "value", { storageClass: "Banana" }), {
    name: "Error", message: "put: We encountered an internal error. Please try again. (10001)",
  });
  await assert.rejects(bucket.delete(Array.from({ length: 1001 }, (_, index) => `key-${index}`)), {
    name: "TypeError", message: "R2_INVALID_OPTIONS",
  });

  for (const limit of [0, -1, 1.5, 1001]) await bucket.list({ limit });
  assert.deepEqual(calls.slice(-4).map(call => call[1].limit), [0, 1000, 1, 1000]);
  await assert.rejects(bucket.list({ include: ["etag"] }), {
    name: "RangeError", message: "Unsupported include value etag",
  });

  const upload = await bucket.createMultipartUpload("multi", { unknown: true });
  await upload.uploadPart(1.5, "value", { unknown: true });
  assert.equal(calls.at(-1)[3], 1);
  await assert.rejects(upload.uploadPart(0, "value"), {
    name: "TypeError",
    message: "Part number must be between 1 and 10000 (inclusive). Actual value was: 0",
  });
  await assert.rejects(Reflect.apply(upload.complete, upload, [{}]), {
    name: "TypeError",
    message: "Failed to execute 'complete' on 'R2MultipartUpload': parameter 1 is not of type 'Array'.",
  });
  assert.equal(bucket.resumeMultipartUpload("multi", "").uploadId, "");
});

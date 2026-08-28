import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const workerBase = moduleUrl("export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }");
const { makeR2TransportBase } = await importRuntime("r2/transport.ts", { "cloudflare:workers": workerBase });
const { R2Bucket } = await importRuntime("r2/facade.ts");
const props = { bindingId: "binding", deploymentId: "deployment", descriptorSha256: "a".repeat(64), resourceSpecGeneration: 1, permissions: { read: true, write: true } };
const meta = { key: "asset", size: 3, etag: "etag", httpEtag: '"etag"', uploaded: 1,
  httpMetadata: { contentType: "text/plain", cacheExpiry: null }, customMetadata: {}, storageClass: "Standard" };
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
  await assert.rejects(object.bytes(), /R2_BODY_ALREADY_USED/);
  const head = await new R2Bucket(transport(async () => Response.json(meta))).head("asset");
  assert.equal(head.httpEtag, '"etag"');
  const list = await transport(async () => Response.json({ objects: [meta], truncated: false, cursor: null, delimitedPrefixes: [] })).list({ prefix: "", limit: 10, include: [] });
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

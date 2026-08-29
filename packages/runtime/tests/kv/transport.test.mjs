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
const props = { bindingId: "binding", deploymentId: "deployment", descriptorSha256: "a".repeat(64),
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
  assert.deepEqual(await kv.getWithMetadata("key", { type: "json", cacheTtl: 30 }), {
    value: { ok: true }, metadata: { owner: "app" },
  });
  assert.deepEqual(calls.at(-1), { operation: "get-with-metadata", request: { keys: ["key"], cacheTtl: 30 } });
  assert.equal(await transport(async () => result(null)).get("missing"), null);
  assert.deepEqual(new Uint8Array(await transport(async () => result([0, 255])).get("binary", "arrayBuffer")), new Uint8Array([0, 255]));
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

test("KV denies undeclared permissions and exposes no echo extension", async () => {
  const kv = transport(async () => { throw new Error("must not reach backend"); }, { read: false, write: false });
  await assert.rejects(kv.get("key"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.put("key", "value"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.delete("key"), /BINDING_PERMISSION_DENIED/);
  await assert.rejects(kv.fetch(), /BINDING_PERMISSION_DENIED/);
  assert.equal(kv.echoStream, undefined);
  const failed = transport(async () => new Response("private details", {
    status: 503, headers: { "x-open-compute-error-code": "KV_RESULT_UNKNOWN" },
  }));
  await assert.rejects(failed.put("key", "value"), { message: "KV_RESULT_UNKNOWN" });
});

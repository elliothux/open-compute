import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const formatUrl = moduleUrl(await compileRuntime("serialization/format.ts"));
const encodeUrl = moduleUrl(await compileRuntime("serialization/encode.ts", { "./format.js": formatUrl }));
const decodeUrl = moduleUrl(await compileRuntime("serialization/decode.ts", { "./format.js": formatUrl }));
const codecUrl = moduleUrl(await compileRuntime("serialization/codec.ts", {
  "./format.js": formatUrl, "./encode.js": encodeUrl, "./decode.js": decodeUrl,
}));
const asyncHooks = moduleUrl(`
  export class AsyncLocalStorage {
    constructor() { this.stack = []; }
    run(store, fn) { this.stack.push(store); try { return fn(); } finally { this.stack.pop(); } }
    getStore() { return this.stack.at(-1); }
  }
`);
const outputGateUrl = moduleUrl(await compileRuntime("durable-objects/output-gate.ts", {
  "node:async_hooks": asyncHooks,
}));
const { DoOutputGate, runWithOutputGate } = await import(outputGateUrl);
const { QueueProducer } = await import(moduleUrl(await compileRuntime("queues/facade.ts", {
  "../serialization/codec.js": codecUrl,
  "../serialization/format.js": formatUrl,
  "../durable-objects/output-gate.js": outputGateUrl,
})));

function frameMessages(bytes) {
  assert.equal(String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]), "OCQ1");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const count = view.getUint16(5);
  let offset = 11;
  const messages = [];
  for (let index = 0; index < count; index++) {
    const contentType = bytes[offset];
    const delay = view.getInt32(offset + 1);
    const length = view.getUint32(offset + 5);
    messages.push({
      contentType,
      delay,
      body: bytes.slice(offset + 9, offset + 9 + length),
    });
    offset += 9 + length;
  }
  return messages;
}

function transport(handler) {
  return {
    send(frame) { return handler("send", frame); },
    sendBatch(frame) { return handler("batch", frame); },
    async finalize(operationId) { await handler("finalize", operationId); },
    metrics() { return handler("metrics"); },
  };
}

function gateStorage() {
  const rows = [];
  return {
    sql: {
      exec(query, ...params) {
        const text = String(query);
        if (text.includes("CREATE TABLE")) return { one() { return {}; }, toArray() { return []; } };
        if (text.includes("INSERT")) {
          const row = {
            id: rows.length + 1,
            kind: params[0],
            publisher: params[1],
            payload: params[2],
            operation_id: params[3],
            state: "pending",
            attempt_count: 0,
            last_error: null,
          };
          rows.push(row);
          return { one() { return { id: row.id }; }, toArray() { return [row]; } };
        }
        if (/SET state = 'published'/i.test(text)) {
          const row = rows.find(value => value.id === Number(params[0]));
          if (row) row.state = "published";
          return { one() { return {}; }, toArray() { return []; } };
        }
        if (text.includes("DELETE")) {
          const index = rows.findIndex(value => value.id === Number(params[0]));
          if (index >= 0) rows.splice(index, 1);
          return { one() { return {}; }, toArray() { return []; } };
        }
        const selected = /WHERE id = \?/i.test(text)
          ? rows.filter(row => row.id === Number(params[0]))
          : rows;
        return { one() { return selected[0] ?? {}; }, toArray() { return [...selected]; } };
      },
    },
    async sync() {},
    rows,
  };
}

test("compiled Queue producer encodes v8 and keeps json/text/bytes exact", async () => {
  const calls = [];
  const queue = new QueueProducer(transport(async (operation, frame) => {
    calls.push({ operation, frame });
    return { backlogCount: 1, backlogBytes: frame?.byteLength ?? 0, oldestMessageTimestampMs: 0 };
  }));
  const json = await queue.send({ ok: true });
  assert.equal(json.metadata.metrics.oldestMessageTimestamp, undefined);
  await queue.send("plain", { contentType: "text", delaySeconds: 0 });
  const bytes = new Uint8Array([1, 2, 3]);
  await queue.send(bytes, { contentType: "bytes" });
  assert.equal(bytes.buffer.byteLength, 0);
  const cycle = { v8: true, when: new Date(1_700_000_000_000) };
  cycle.self = cycle;
  cycle.items = new Map([["k", new Set([1])]]);
  await queue.send(cycle, { contentType: "v8", delaySeconds: 9 });
  assert.equal(frameMessages(calls[0].frame)[0].contentType, 1);
  assert.equal(frameMessages(calls[1].frame)[0].contentType, 2);
  assert.equal(frameMessages(calls[2].frame)[0].contentType, 3);
  const encoded = frameMessages(calls[3].frame)[0];
  assert.equal(encoded.contentType, 4);
  assert.equal(encoded.delay, 9);
  assert.deepEqual([...encoded.body.slice(0, 4)], [0x4f, 0x43, 0x44, 0x56]);
  await queue.sendBatch([{ body: "a", contentType: "text" }, { body: 1 }], { delaySeconds: 3 });
  assert.equal(calls[4].operation, "batch");
});

test("compiled Queue producer rejects unsupported, oversized, delayed, and detached values", async () => {
  const queue = new QueueProducer(transport(async () => ({
    backlogCount: 0, backlogBytes: 0, oldestMessageTimestampMs: null,
  })));
  await assert.rejects(queue.send("x", { contentType: "xml" }), { name: "TypeError", message: "QUEUE_CONTENT_TYPE_UNSUPPORTED" });
  await assert.rejects(queue.send(undefined), { name: "TypeError", message: "QUEUE_INVALID_MESSAGE" });
  await assert.rejects(queue.send("x", { delaySeconds: -1 }), { name: "Error", message: "QUEUE_DELAY_INVALID" });
  await assert.rejects(queue.send("x", { delaySeconds: 86_401 }), { name: "Error", message: "QUEUE_DELAY_INVALID" });
  await assert.rejects(queue.send(new Uint8Array(128_001), { contentType: "bytes" }), {
    name: "TypeError", message: "QUEUE_MESSAGE_TOO_LARGE",
  });
  await assert.rejects(queue.sendBatch([]), { name: "TypeError", message: "QUEUE_INVALID_MESSAGE" });
  await assert.rejects(queue.sendBatch(Array.from({ length: 101 }, () => ({ body: "x" }))), {
    name: "Error", message: "QUEUE_BATCH_LIMIT_EXCEEDED",
  });
  await assert.rejects(queue.send(Promise.resolve(1), { contentType: "v8" }), {
    name: "TypeError", message: "QUEUE_V8_UNSUPPORTED",
  });
  const detached = new Uint8Array(new ArrayBuffer(4));
  detached.buffer.transfer?.(0);
  if (detached.buffer.detached === true) {
    await assert.rejects(queue.send(detached, { contentType: "bytes" }), {
      name: "TypeError", message: "QUEUE_INVALID_MESSAGE",
    });
  }
  const doQueue = new QueueProducer(transport(async () => ({ backlogCount: 0, backlogBytes: 0 })), true);
  await assert.rejects(doQueue.send("x"), { name: "TypeError", message: "QUEUE_INVARIANT_VIOLATION" });
});

test("awaited Durable Object Queue send stages, commits, finalizes, and removes its intent", async () => {
  const storage = gateStorage();
  const gate = new DoOutputGate(storage);
  const operations = [];
  const raw = {
    async send(_frame, operationId) {
      operations.push(["send", operationId]);
      return { backlogCount: 4, backlogBytes: 7 };
    },
    async sendBatch() { throw new Error("unexpected batch"); },
    async finalize(operationId) { operations.push(["finalize", operationId]); },
    async metrics() { return { backlogCount: 3, backlogBytes: 4 }; },
  };
  const queue = new QueueProducer(raw, true, "EVENTS");
  gate.enterTransaction();
  const staged = await runWithOutputGate(gate, () => queue.send("x", { contentType: "text" }));
  assert.equal(staged.metadata.metrics.backlogCount, 4);
  assert.equal(staged.metadata.metrics.backlogBytes, 5);
  assert.deepEqual(operations, []);
  await gate.exitTransaction("committed");
  assert.equal(operations.length, 2);
  assert.equal(operations[0][0], "send");
  assert.equal(operations[1][0], "finalize");
  assert.equal(operations[0][1], operations[1][1]);
  assert.equal(storage.rows.length, 0);
});

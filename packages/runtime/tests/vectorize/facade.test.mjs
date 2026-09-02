import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { VectorizeBinding } = await importRuntime("vectorize/facade.ts");

test("latest Vectorize surface normalizes pinned options and emits the bounded mutation frame", async () => {
  const calls = []; let frame;
  const binding = new VectorizeBinding({
    async call(operation, payload) {
      calls.push({ operation, payload });
      if (operation === "describe") return { vectorCount: 1, dimensions: 2, processedUpToDatetime: 3, processedUpToMutation: 4 };
      if (operation === "query" || operation === "queryById") return { matches: [{ id: "one", score: 0.9, values: [1, 2] }], count: 1 };
      if (operation === "getByIds") return [{ id: "one", values: [1, 2] }];
      return { mutationId: "mutation-1" };
    },
    async mutate(_operation, stream) {
      frame = new Uint8Array(await new Response(stream).arrayBuffer());
      return { mutationId: "mutation-1" };
    },
  });
  assert.equal((await binding.describe()).dimensions, 2);
  assert.equal((await binding.query(new Float64Array([1 / 3, 2]), { topK: 50, returnMetadata: true })).count, 1);
  assert.equal(calls[1].payload.options.returnMetadata, "all");
  assert.equal(calls[1].payload.vector[0], Math.fround(1 / 3));
  assert.equal((await binding.queryById("one", { filter: { year: { $gte: 2020, $lt: 2030 } } })).matches[0].id, "one");
  assert.equal((await binding.insert([{ id: "one", values: [1, 2], namespace: "docs", metadata: { title: "One" } }])).mutationId, "mutation-1");
  assert.equal(new TextDecoder().decode(frame.subarray(0, 4)), "OCVZ");
  assert.equal(new DataView(frame.buffer).getUint16(4, false), 1);
  assert.equal((await binding.deleteByIds(["one"])).mutationId, "mutation-1");
  assert.equal((await binding.getByIds(["one"]))[0].id, "one");
  assert.equal("queryById" in binding, true);
  assert.equal("VectorizeIndex" in binding, false);
});

test("Vectorize rejects unsupported options, invalid ranges, limits, and malformed backend success", async () => {
  const malformed = new VectorizeBinding({ async call() { return { matches: [], count: 1 }; }, async mutate() { return {}; } });
  await assert.rejects(malformed.query([1]), /VECTORIZE_INPUT_INVALID|VECTORIZE_PROTOCOL_ERROR/);
  const binding = new VectorizeBinding({ async call() { return { matches: [], count: 0 }; }, async mutate() { return { mutationId: "ok" }; } });
  await assert.rejects(binding.query([1], { topK: 51, returnValues: true }), /VECTORIZE_LIMIT_EXCEEDED/);
  await assert.rejects(binding.query([1], { filter: {} }), /VECTORIZE_INPUT_INVALID/);
  await assert.rejects(binding.query([1], { filter: { year: { $gt: 1, $gte: 2 } } }), /VECTORIZE_INPUT_INVALID/);
  await assert.rejects(binding.insert([]), /VECTORIZE_LIMIT_EXCEEDED/);
});

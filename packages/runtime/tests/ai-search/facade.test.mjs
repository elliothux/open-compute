import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { AiSearchNamespaceBinding, AiSearchInstanceBinding } = await importRuntime("ai-search/facade.ts");
const instance = { id: "docs", status: "ready" };
const item = { id: "item-1", key: "guide.txt", status: "completed" };
const job = { id: "job-1", source: "user" };

function transport(calls) {
  return {
    async call(operation, selected, payload) {
      calls.push({ operation, selected, payload });
      if (operation === "namespace.list") return { result: [instance], result_info: { count: 1, page: 1, per_page: 10, total_count: 1 } };
      if (operation === "namespace.create" || operation.endsWith(".info") || operation === "instance.update") return operation.startsWith("item.") ? item : operation.startsWith("job.") ? job : instance;
      if (operation.endsWith(".search")) return { search_query: "cache", chunks: [] };
      if (operation.endsWith(".chatCompletions")) return { choices: [{ message: { role: "assistant", content: "answer" } }], chunks: [] };
      if (operation === "instance.stats") return { completed: 1 };
      if (operation === "items.list") return { result: [item], result_info: { count: 1, page: 1, per_page: 10, total_count: 1 } };
      if (operation === "items.delete" || operation === "namespace.delete") return null;
      if (operation === "item.logs") return { result: [], result_info: { count: 0, per_page: 50, cursor: null, truncated: false } };
      if (operation === "item.chunks") return { result: [], result_info: { count: 0, total: 0, limit: 20, offset: 0 } };
      if (operation === "jobs.list") return { result: [job], result_info: { count: 1, page: 1, per_page: 10, total_count: 1 } };
      if (operation === "jobs.create" || operation === "job.cancel") return job;
      if (operation === "job.logs") return { result: [], result_info: { count: 0, page: 1, per_page: 10, total_count: 0 } };
      throw new Error(`unexpected ${operation}`);
    },
    async stream() { return new Response("data: ok\n\n", { headers: { "content-type": "text/event-stream" } }); },
    async upload(_selected, _name, contentType, _body, options) { calls.push({ operation: "upload", contentType, options }); return item; },
    async download() { return new Response("guide", { headers: { "content-type": "text/plain", "content-length": "5", "x-open-compute-filename": "guide.txt" } }); },
  };
}

test("AI Search namespace, instance, item, job, upload, download, and stream surfaces are reachable", async () => {
  const calls = []; const raw = transport(calls); const namespace = new AiSearchNamespaceBinding(raw);
  assert.equal(namespace.get("docs") instanceof AiSearchInstanceBinding, true);
  assert.equal((await namespace.list({ order_by: "created_at", order_by_direction: "asc" })).result[0].id, "docs");
  assert.equal((await namespace.create({ id: "new-docs", index_method: { vector: true } })).constructor, AiSearchInstanceBinding);
  assert.equal((await namespace.search({ query: "cache", ai_search_options: { instance_ids: ["docs"] } })).search_query, "cache");
  assert.ok(await namespace.chatCompletions({ messages: [{ role: "user", content: "cache" }], stream: true, ai_search_options: { instance_ids: ["docs"] } }) instanceof ReadableStream);
  const direct = new AiSearchInstanceBinding(raw);
  assert.equal((await direct.search({ query: "cache" })).chunks.length, 0);
  assert.equal((await direct.info()).id, "docs"); assert.equal((await direct.stats()).completed, 1);
  assert.equal((await direct.items.list()).result[0].id, "item-1");
  assert.equal((await direct.items.upload("guide.txt", "guide", { metadata: { rank: "2" } })).id, "item-1");
  assert.equal((await direct.items.get("item-1").download()).filename, "guide.txt");
  assert.equal((await direct.items.get("item-1").logs()).result.length, 0);
  assert.equal((await direct.items.get("item-1").chunks()).result.length, 0);
  assert.equal((await direct.jobs.list()).result[0].id, "job-1");
  assert.equal((await direct.jobs.create({ description: "refresh" })).id, "job-1");
  assert.equal((await direct.jobs.get("job-1").cancel()).id, "job-1");
  assert.equal(calls.find(call => call.operation === "upload").contentType, "text/plain");
  assert.deepEqual(calls.find(call => call.operation === "upload").options, { metadata: { rank: "2" } });
});

test("AI Search rejects unknown options, limits, unsupported first tranche, and malformed backend success", async () => {
  const raw = transport([]); const direct = new AiSearchInstanceBinding(raw);
  await assert.rejects(direct.search({ query: "x", extra: true }), /AI_SEARCH_INPUT_INVALID/);
  await assert.rejects(direct.search({ query: "x", ai_search_options: { retrieval: { max_num_results: 51 } } }), /AI_SEARCH_INPUT_INVALID/);
  await assert.rejects(direct.search({ query: "x", ai_search_options: { retrieval: { boost_by: [{ field: "x" }] } } }), /AI_SEARCH_OPTION_UNSUPPORTED/);
  await assert.rejects(direct.update({ chunk_overlap: 31 }), /AI_SEARCH_INPUT_INVALID/);
  assert.equal((await direct.update({ embedding_model: "@cf/baai/bge-large-en-v1.5" })).id, "docs");
  await assert.rejects(direct.items.upload("guide.txt", "guide", { metadata: { rank: 2 } }), /AI_SEARCH_INPUT_INVALID/);
  const malformed = new AiSearchInstanceBinding({ ...raw, async call() { return { choices: [] }; } });
  await assert.rejects(malformed.search({ query: "x" }), /AI_SEARCH_INPUT_INVALID|AI_SEARCH_PROTOCOL_ERROR/);
  const badStream = new AiSearchInstanceBinding({ ...raw, async stream() { return new Response("x"); } });
  await assert.rejects(badStream.chatCompletions({ messages: [{ role: "user", content: "x" }], stream: true }), /AI_SEARCH_PROTOCOL_ERROR/);
});

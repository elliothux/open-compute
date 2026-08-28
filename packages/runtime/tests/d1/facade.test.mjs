import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { D1Database } = await importRuntime("d1/facade.ts");
const meta = {
  served_by: "open-compute-local", served_by_primary: true, duration: 0,
  changes: 0, last_row_id: 0, changed_db: false, size_after: 0,
  rows_read: 1, rows_written: 0,
};

test("compiled D1 facade normalizes bindings and returns validated rows", async () => {
  const calls = [];
  const db = new D1Database({
    async query(mode, statements) {
      calls.push({ mode, statements });
      return { results: [{ columns: ["value"], rows: [[new Uint8Array([1, 2])]], meta }] };
    },
    async exec() { return { count: 1, duration: 0 }; },
  });
  const prepared = db.prepare("select ? as value").bind(true);
  assert.deepEqual(await prepared.first(), { value: [1, 2] });
  assert.deepEqual(calls[0], { mode: "all", statements: [{ sql: "select ? as value", params: [1] }] });
  assert.deepEqual(await db.exec("create table entries(value)"), { count: 1, duration: 0 });
  assert.equal(db.withSession().getBookmark(), null);
  await assert.rejects(db.batch([db.withSession().prepare("select 1")]), /D1_INVALID_BATCH/);
  assert.throws(() => prepared.bind(Number.NaN), /D1_TYPE_ERROR/);
});

test("compiled D1 facade rejects malformed results without exposing them", async () => {
  for (const result of [
    null,
    { results: [{ columns: [42], rows: [["private"]], meta }] },
    { results: [{ columns: ["value"], rows: [], meta: { ...meta, privateToken: "secret" } }] },
    { results: [{ columns: ["value"], rows: [[Number.NaN]], meta }] },
    { results: [] },
  ]) {
    const db = new D1Database({ async query() { return result; }, async exec() {} });
    await assert.rejects(db.prepare("select 1").all(), { name: "TypeError", message: "D1_INTERNAL_PROTOCOL_ERROR" });
  }
});

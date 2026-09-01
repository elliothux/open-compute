import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { D1Database } = await importRuntime("d1/facade.ts");
const meta = {
  duration: 0, size_after: 0, rows_read: 1, rows_written: 0,
  last_row_id: 0, changed_db: false, changes: 0, served_by_primary: true,
  timings: { sql_duration_ms: 0 }, total_attempts: 1,
};

function result(columns = ["value"], rows = [[1]], session = { kind: 0 }, version = 0) {
  return {
    results: [{ columns, rows, meta }],
    bookmark: session.kind === 0 ? null : `token-${version}`,
    stateVersion: version,
  };
}

test("compiled D1 facade normalizes values and returns Cloudflare result shapes", async () => {
  const calls = [];
  const db = new D1Database({
    async query(mode, statements, session) {
      calls.push({ mode, statements, session });
      return result(["value"], [[new Uint8Array([1, 2])]], session);
    },
    async exec() { return { count: 1, duration: 0 }; },
  });
  const prepared = db.prepare("select ? as value").bind(true);
  assert.deepEqual(await prepared.first(), { value: [1, 2] });
  assert.deepEqual(calls[0], {
    mode: "all",
    statements: [{ sql: "select ? as value", params: [1] }],
    session: { kind: 0 },
  });
  assert.deepEqual(await db.exec("create table entries(value)"), { count: 1, duration: 0 });
  assert.equal(db.withSession().getBookmark(), null);
});

test("compiled D1 bind follows the pinned workerd conversions and errors", async () => {
  const calls = [];
  const db = new D1Database({
    async query(mode, statements, session) {
      calls.push({ mode, statements, session });
      return result(["value"], [[null]], session);
    },
    async exec() { return { count: 1, duration: 0 }; },
  });
  const statement = db.prepare("select ?");
  assert.throws(
    () => statement.bind(undefined),
    error => error.name === "Error"
      && error.message === "D1_TYPE_ERROR: Type 'undefined' not supported for value 'undefined'"
      && error.cause?.message === "Type 'undefined' not supported for value 'undefined'",
  );
  assert.throws(() => statement.bind({}), /D1_TYPE_ERROR: Type 'object' not supported/);
  assert.throws(() => statement.bind(Symbol("x")), TypeError);
  await statement.bind(Number.NaN, Number.POSITIVE_INFINITY, Number.MAX_SAFE_INTEGER + 1).all();
  assert.deepEqual(calls.at(-1).statements[0].params, [null, null, 9007199254740992]);
  await statement.bind(new Uint16Array([1, 2]), new DataView(new ArrayBuffer(2)), new Int16Array([256, -1])).all();
  assert.deepEqual(Array.from(calls.at(-1).statements[0].params[0]), [1, 2]);
  assert.deepEqual(Array.from(calls.at(-1).statements[0].params[1]), []);
  assert.equal(calls.at(-1).statements[0].params[2], "256,-1");
  await statement.bind([1.5]).all();
  assert.deepEqual(Array.from(calls.at(-1).statements[0].params[0]), [1]);
});

test("compiled D1 batch accepts prepared statements from other bindings and rejects malformed input", async () => {
  const calls = [];
  const raw = {
    async query(mode, statements, session) {
      calls.push({ mode, statements, session });
      return {
        results: statements.map(() => result().results[0]),
        bookmark: session.kind === 0 ? null : "token-1",
        stateVersion: 1,
      };
    },
    async exec() { return { count: 1, duration: 0 }; },
  };
  const db = new D1Database(raw);
  const other = new D1Database({ async query() { throw new Error("wrong transport"); }, async exec() {} });
  const foreign = other.prepare("select 1");
  assert.equal((await db.batch([foreign]))[0].success, true);
  assert.equal(calls[0].statements[0].sql, "select 1");
  await assert.rejects(db.batch([]), {
    name: "Error",
    message: "D1_ERROR: No SQL statements detected.",
  });
  await assert.rejects(db.batch([{}]), /D1_ERROR: Malformed input: \[{}\]/);
});

test("compiled D1 raw and first preserve the pinned wrapper's permissive call behavior", async () => {
  const db = new D1Database({
    async query(_mode, _statements, session) {
      return result(["value", "value"], [[1, 2]], session);
    },
    async exec() { return { count: 1, duration: 0 }; },
  });
  const prepared = db.prepare("select 1 as value, 2 as value");
  assert.equal(await prepared.first("value", "ignored"), 2);
  await assert.rejects(prepared.first(42), error => error.name === "Error"
    && error.message === "D1_COLUMN_NOTFOUND: Column not found (42)"
    && error.cause?.message === "Column not found");
  assert.deepEqual(await prepared.raw(null), [[1, 2]]);
  assert.deepEqual(await prepared.raw({ extra: true }), [[1, 2]]);
  assert.deepEqual(await prepared.raw({ columnNames: true }), [["value", "value"], [1, 2]]);
});

test("compiled D1 session trims constraints and observes opaque bookmarks", async () => {
  let version = 0;
  const db = new D1Database({
    async query(mode, statements, session) {
      if (session?.kind === 3 && session.bookmark === "future") {
        throw Object.assign(new Error("D1_SESSION_ERROR"), { stableCode: "D1_SESSION_ERROR" });
      }
      version += 1;
      return result(["value"], [[version]], session, version);
    },
    async exec() { return { count: 1, duration: 0 }; },
  });
  assert.equal(db.withSession(null).getBookmark(), null);
  assert.equal(db.withSession("   ").getBookmark(), null);
  assert.equal(db.withSession(" token ").getBookmark(), "token");
  const session = db.withSession("first-primary");
  assert.equal(session.getBookmark(), null);
  assert.deepEqual(await session.prepare("select 1").first("value"), 1);
  const bookmark = session.getBookmark();
  assert.equal(bookmark, "token-1");
  const resumed = db.withSession(bookmark);
  assert.equal(resumed.getBookmark(), "token-1");
  assert.equal((await resumed.prepare("select 1").all()).results[0].value, 2);
  assert.equal(resumed.getBookmark(), "token-2");
  await assert.rejects(db.withSession("future").prepare("select 1").all(), /D1_ERROR: D1_SESSION_ERROR/);
});

test("compiled D1 facade exposes pinned empty-SQL and dump failures", async () => {
  const db = new D1Database({
    async query() { throw new Error("unexpected query"); },
    async exec() { throw new Error("unexpected exec"); },
  });
  await assert.rejects(db.prepare("").all(), error => error.name === "Error"
    && error.message === "D1_ERROR: No SQL statements detected."
    && error.cause?.message === "No SQL statements detected.");
  for (const sql of ["", "   "]) {
    await assert.rejects(db.exec(sql), error => error.name === "Error"
      && error.message === "D1_EXEC_ERROR: Error in line 1: : No SQL statements detected."
      && error.cause?.message === "Error in line 1: : No SQL statements detected.");
  }
  await assert.rejects(db.dump(), error => error.name === "Error"
    && error.message === "D1_DUMP_ERROR: Status + 400"
    && error.cause?.message === "Status 400");
});

test("compiled D1 facade maps private failures but rejects malformed results without exposing them", async () => {
  const failed = new D1Database({
    async query() { throw Object.assign(new Error("D1_LIMIT_ERROR"), { stableCode: "D1_LIMIT_ERROR" }); },
    async exec() { throw Object.assign(new Error("D1_SQL_INVALID"), { stableCode: "D1_SQL_INVALID" }); },
  });
  await assert.rejects(failed.prepare("select 1").all(), /D1_ERROR: D1_LIMIT_ERROR/);
  await assert.rejects(failed.exec("select 1"), /D1_EXEC_ERROR: D1_SQL_INVALID/);

  for (const malformed of [
    null,
    { results: [{ columns: [42], rows: [["private"]], meta }] },
    { results: [{ columns: ["value"], rows: [], meta: { ...meta, privateToken: "secret" } }] },
    { results: [{ columns: ["value"], rows: [[Number.NaN]], meta }] },
    { results: [] },
  ]) {
    const db = new D1Database({ async query() { return malformed; }, async exec() {} });
    await assert.rejects(db.prepare("select 1").all(), { name: "TypeError", message: "D1_INTERNAL_PROTOCOL_ERROR" });
  }
});

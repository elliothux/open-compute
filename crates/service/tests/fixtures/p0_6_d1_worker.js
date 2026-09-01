import { WorkerEntrypoint } from "cloudflare:workers";
import { D1Database as ImportableD1Database } from "./__open_compute__/d1/facade.js";
import { R2Bucket as ImportableR2Bucket } from "./__open_compute__/r2/facade.js";

const meta = () => ({
  duration: 0, size_after: 0, rows_read: 1, rows_written: 0,
  last_row_id: 0, changed_db: false, changes: 0, served_by_primary: true,
  timings: { sql_duration_ms: 0 }, total_attempts: 1,
});
const fakeResult = (columns = ["value"], rows = [[1]]) => ({ results: [{ columns, rows, meta: meta() }], bookmark: null, stateVersion: 0 });
const codeOf = (error) => String(error && error.message || error);
const syncThrows = (fn, code) => {
  try { fn(); return false; } catch (error) { return codeOf(error).includes(code); }
};
const rejects = async (fn, code) => {
  try { await fn(); return false; } catch (error) { return codeOf(error).includes(code); }
};

export class Named extends WorkerEntrypoint {
  constructor(ctx, env) { super(ctx, env); this.wrapped = env.DB instanceof ImportableD1Database; }
  async fetch() { return new Response(`named:${this.wrapped}`); }
}

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/dump") {
      try { await env.DB.dump(); return new Response("false"); }
      catch (error) { return new Response(String(codeOf(error).includes("D1_DUMP_ERROR"))); }
    }
    if (path === "/session") {
      const unconstrained = env.DB.withSession();
      const primary = env.DB.withSession("first-primary");
      const before = unconstrained.getBookmark() === null && primary.getBookmark() === null;
      await unconstrained.prepare("SELECT count(*) AS n FROM items").first("n");
      const bookmark = unconstrained.getBookmark();
      const opaque = typeof bookmark === "string" && bookmark.length > 0 && !/^[0-9]+$/.test(bookmark);
      const resumed = env.DB.withSession(bookmark);
      const seen = await resumed.prepare("SELECT count(*) AS n FROM items").first("n");
      const afterResume = typeof resumed.getBookmark() === "string" && resumed.getBookmark().length > 0;
      let invalid = false;
      try { await env.DB.withSession("%%%not-a-bookmark%%%").prepare("SELECT 1").all(); }
      catch (error) { invalid = codeOf(error).includes("D1_SESSION_ERROR"); }
      let otherDb = false;
      try { await env.OTHER.withSession(bookmark).prepare("SELECT 1").all(); }
      catch (error) { otherDb = codeOf(error).includes("D1_SESSION_ERROR"); }
      const firstRow = await env.DB.prepare("SELECT id, value FROM items ORDER BY id").first();
      const firstCol = await env.DB.prepare("SELECT id, value FROM items ORDER BY id").first("value");
      const rawNamed = await env.DB.prepare("SELECT id, value FROM items ORDER BY id").raw({ columnNames: true });
      const rawPlain = await env.DB.prepare("SELECT id, value FROM items ORDER BY id").raw();
      const runMeta = (await env.DB.prepare("SELECT 1 AS value").run()).meta;
      const metaShape = runMeta && typeof runMeta.duration === "number" && typeof runMeta.size_after === "number"
        && typeof runMeta.rows_read === "number" && typeof runMeta.rows_written === "number"
        && typeof runMeta.last_row_id === "number" && typeof runMeta.changed_db === "boolean"
        && typeof runMeta.changes === "number" && !("served_by" in runMeta)
        && (runMeta.served_by_primary === undefined || runMeta.served_by_primary === true)
        && (runMeta.timings === undefined || typeof runMeta.timings.sql_duration_ms === "number")
        && (runMeta.total_attempts === undefined || typeof runMeta.total_attempts === "number");
      return Response.json({
        before, opaque, seen, afterResume, invalid, otherDb, bookmark,
        firstRow: firstRow && firstRow.id === 1, firstCol, rawNamed: Array.isArray(rawNamed) && rawNamed[0][0] === "id",
        rawPlain: Array.isArray(rawPlain) && rawPlain[0][0] === 1, metaShape,
      });
    }
    if (path === "/resume") {
      const bookmark = new URL(request.url).searchParams.get("b");
      const session = env.DB.withSession(bookmark);
      const n = await session.prepare("SELECT count(*) AS n FROM items").first("n");
      return Response.json({ n, bookmark: session.getBookmark() });
    }
    if (path === "/count") return new Response(String((await env.DB.prepare("SELECT count(*) AS n FROM items").first("n"))));
    if (path === "/batch-loss") {
      try {
        await env.DB.batch([
          env.DB.prepare("INSERT INTO items(value) VALUES ('lost-batch-a')"),
          env.DB.prepare("INSERT INTO items(value) VALUES ('lost-batch-b')"),
        ]);
        return new Response("false");
      } catch (error) {
        const committed = (await env.DB.prepare(
          "SELECT count(*) AS n FROM items WHERE value IN ('lost-batch-a', 'lost-batch-b')",
        ).first("n")) === 2;
        await env.DB.prepare(
          "DELETE FROM items WHERE value IN ('lost-batch-a', 'lost-batch-b')",
        ).run();
        return new Response(String(codeOf(error).includes("D1_RESULT_UNKNOWN") && committed));
      }
    }
    if (path !== "/matrix") return new Response("missing", { status: 404 });
    try {
      const calls = [];
      const raw = {
        async query(mode, statements) {
          calls.push({ mode, statements });
          if (mode === "batch") {
            return { results: statements.map(() => fakeResult().results[0]), bookmark: null, stateVersion: 0 };
          }
          if (statements[0].sql === "magic") return fakeResult(["__proto__", "constructor"], [["safe", "also-safe"]]);
          if (statements[0].sql === "empty") return fakeResult(["value"], []);
          return fakeResult();
        },
        async exec(sql) { calls.push({ exec: sql }); return { count: 1, duration: 0 }; },
      };
      const fake = new ImportableD1Database(raw);
      const prepared = fake.prepare("SELECT ?1");
      const df01 = prepared && calls.length === 0;
      const view = new Uint8Array([9, 1, 2, 9]).subarray(1, 3);
      const bound = prepared.bind(view);
      const reused = prepared.bind("again");
      const df02 = bound !== prepared && reused !== prepared && calls.length === 0;
      await bound.all();
      const df03 = calls.length === 1 && calls[0].mode === "all"
        && calls[0].statements.length === 1 && calls[0].statements[0].params[0] instanceof Uint8Array;
      const beforeBatch = calls.length;
      const batch = await fake.batch([bound, reused, bound]);
      const df04 = batch.length === 3 && calls.length === beforeBatch + 1
        && calls.at(-1).statements.length === 3;
      const other = new ImportableD1Database(raw);
      const session = fake.withSession("first-primary");
      const df05 = (await fake.batch([other.prepare("SELECT 1"), session.prepare("SELECT 2")])).length === 2
        && await rejects(() => fake.batch([{}]), "D1_ERROR: Malformed input");
      const rejected = [undefined, 1n, {}, new Date(), () => {}]
        .every((value) => syncThrows(() => prepared.bind(value), "D1_TYPE_ERROR"));
      let symbolMatchesWorkerd = false;
      try {
        prepared.bind(Symbol("x"));
      } catch (error) {
        const message = codeOf(error).toLowerCase();
        symbolMatchesWorkerd = error instanceof TypeError
          && message.includes("cannot convert") && message.includes("symbol") && message.includes("string");
      }
      const df06 = rejected && symbolMatchesWorkerd
        && prepared.bind(null, true, false, 1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, "x", [1.5], new ArrayBuffer(0));
      const df07 = calls[0].statements[0].params[0].byteLength === 2
        && calls[0].statements[0].params[0][0] === 1 && calls[0].statements[0].params[0][1] === 2;
      const magic = await fake.prepare("magic").first();
      const df08 = Object.getPrototypeOf(magic) === Object.prototype
        && Object.prototype.hasOwnProperty.call(magic, "__proto__") && magic.__proto__ === "safe";
      const df09 = env.DB instanceof ImportableD1Database && env.DB_ALIAS instanceof ImportableD1Database;
      const df10 = Object.keys(env).sort().join(",") === "BUCKET,DB,DB_ALIAS,OTHER"
        && !Reflect.ownKeys(env.DB).some((key) => String(key).includes("raw"))
        && typeof env.DB.fetch === "undefined";
      const df11 = df09;
      const df12 = env.BUCKET instanceof ImportableR2Bucket
        && typeof env.BUCKET.head === "function" && typeof env.BUCKET.fetch === "undefined"
        && await env.BUCKET.head("missing") === null;

      let resultUnknown = false;
      try {
        await env.DB.exec("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT UNIQUE, data BLOB)");
      } catch (error) {
        resultUnknown = codeOf(error).includes("D1_RESULT_UNKNOWN")
          && (await env.DB.prepare("SELECT count(*) AS n FROM sqlite_master WHERE type='table' AND name='items'").first("n")) === 1;
      }
      await env.DB.prepare("INSERT INTO items(value, data) VALUES (?1, ?2)").bind("one", view).run();
      await env.DB.batch([
        env.DB.prepare("INSERT INTO items(value) VALUES (?1)").bind("two"),
        env.DB.prepare("SELECT count(*) FROM items"),
      ]);
      const real = await env.DB.prepare("SELECT id, value FROM items ORDER BY id").raw();
      const blob = await env.DB.prepare("SELECT data FROM items WHERE id = 1").first("data");
      let batchRollback = false;
      try {
        await env.DB.batch([
          env.DB.prepare("INSERT INTO items(value) VALUES ('three')"),
          env.DB.prepare("INSERT INTO items(value) VALUES ('one')"),
        ]);
      } catch {
        batchRollback = (await env.DB.prepare("SELECT count(*) AS n FROM items WHERE value='three'").first("n")) === 0;
      }
      let execPrefix = false;
      try {
        await env.DB.exec("INSERT INTO items(value) VALUES ('prefix'); SELECT * FROM absent; INSERT INTO items(value) VALUES ('never')");
      } catch {
        execPrefix = (await env.DB.prepare("SELECT count(*) AS n FROM items WHERE value='prefix'").first("n")) === 1;
        await env.DB.prepare("DELETE FROM items WHERE value='prefix'").run();
      }
      const denied = [];
      for (const sql of ["ATTACH DATABASE ':memory:' AS other", "PRAGMA writable_schema=ON", "BEGIN", "SELECT * FROM __open_compute_meta"]) {
        try { await env.DB.exec(sql); denied.push(false); } catch (error) { denied.push(codeOf(error).includes("D1_AUTHORIZER_DENIED")); }
      }
      const firstNull = await fake.prepare("empty").first() === null;
      const rawColumns = JSON.stringify(await fake.prepare("magic").raw({ columnNames: true }))
        === JSON.stringify([["__proto__", "constructor"], ["safe", "also-safe"]]);
      const sqlLimit = await rejects(
        () => env.DB.prepare("x".repeat(100001)).all(), "D1_ERROR: D1_LIMIT_ERROR",
      );
      const parameterLimit = await rejects(
        () => env.DB.prepare("SELECT 1").bind(...Array(101).fill(null)).all(), "D1_LIMIT_ERROR",
      );
      const rowLimit = await rejects(
        () => env.DB.prepare("SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3").all(),
        "D1_LIMIT_ERROR",
      );
      const resultLimit = await rejects(
        () => env.DB.prepare("SELECT printf('%2000s', 'x')").all(), "D1_LIMIT_ERROR",
      );
      const vmLimit = await rejects(
        () => env.DB.prepare("WITH RECURSIVE c(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM c WHERE x<100000) SELECT sum(x) FROM c").all(),
        "D1_LIMIT_ERROR",
      );
      const limitMatrix = sqlLimit && parameterLimit
        && rowLimit && resultLimit && vmLimit;
      return Response.json({
        df01, df02, df03, df04, df05, df06: Boolean(df06), df07, df08, df09, df10, df11, df12,
        realRows: real, blob, batchRollback, execPrefix, authorizer: denied.every(Boolean),
        resultUnknown, firstNull, rawColumns, limitMatrix,
      });
    } catch (error) {
      return new Response(error && error.stack ? error.stack : String(error), { status: 598 });
    }
  }
};

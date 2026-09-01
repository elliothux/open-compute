interface Env {
  DB: D1Database;
  OTHER: D1Database;
}

interface ErrorObservation {
  synchronous: boolean;
  name: string;
  message: string;
}

function invoke(method: Function, owner: object, args: unknown[]): unknown {
  return Reflect.apply(method, owner, args);
}

async function capture(call: () => unknown): Promise<ErrorObservation | null> {
  let synchronous = true;
  try {
    const pending = call();
    synchronous = false;
    await pending;
    return null;
  } catch (error) {
    return {
      synchronous,
      name: error instanceof Error ? error.name : typeof error,
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

function validMeta(meta: D1Meta): boolean {
  return typeof meta.duration === "number"
    && typeof meta.size_after === "number"
    && typeof meta.rows_read === "number"
    && typeof meta.rows_written === "number"
    && typeof meta.last_row_id === "number"
    && typeof meta.changed_db === "boolean"
    && typeof meta.changes === "number"
    && (meta.served_by_region === undefined || typeof meta.served_by_region === "string")
    && (meta.served_by_colo === undefined || typeof meta.served_by_colo === "string")
    && (meta.served_by_primary === undefined || typeof meta.served_by_primary === "boolean")
    && (meta.timings === undefined || typeof meta.timings.sql_duration_ms === "number")
    && (meta.total_attempts === undefined || typeof meta.total_attempts === "number");
}

async function reset(env: Env): Promise<Response> {
  const sql = "DROP TABLE IF EXISTS portable;\nCREATE TABLE portable(id INTEGER PRIMARY KEY, value TEXT UNIQUE, data BLOB);";
  const db = await env.DB.exec(sql);
  const other = await env.OTHER.exec(sql);
  return Response.json({ reset: db.count === 2 && other.count === 2 });
}

async function surface(env: Env): Promise<Response> {
  const local = env.DB.prepare("INSERT INTO portable(value, data) VALUES (?, ?)")
    .bind("one", new Uint16Array([1, 2]));
  const foreign = env.OTHER.prepare("INSERT INTO portable(value, data) VALUES (?, ?)")
    .bind("two", new Uint8Array([3, 4]));
  const batch = await env.DB.batch([local, foreign]);
  const prepared = env.DB.prepare("SELECT id, value, data FROM portable ORDER BY id");
  const all = await prepared.all<{ id: number; value: string; data: number[] }>();
  const run = await prepared.run<{ id: number; value: string; data: number[] }>();
  const first = await prepared.first<{ id: number; value: string; data: number[] }>();
  const firstColumn = await prepared.first<string>("value");
  const duplicateColumn = await env.DB.prepare("SELECT 1 AS duplicate, 2 AS duplicate").first<number>("duplicate");
  const raw = await prepared.raw<[number, string, number[]]>();
  const rawNames = await prepared.raw<[number, string, number[]]>({ columnNames: true });
  const exec = await env.DB.exec("SELECT 1");
  const otherCount = await env.OTHER.prepare("SELECT count(*) AS n FROM portable").first<number>("n");
  return Response.json({
    rows: all.results,
    raw,
    rawNames,
    first,
    firstColumn,
    duplicateColumn,
    batch: batch.map(item => ({ success: item.success, results: item.results, meta: validMeta(item.meta) })),
    allShape: all.success && !Object.hasOwn(all, "error") && validMeta(all.meta),
    runShape: run.success && !Object.hasOwn(run, "error") && validMeta(run.meta)
      && JSON.stringify(run.results) === JSON.stringify(all.results),
    execShape: exec.count === 1 && typeof exec.duration === "number",
    foreignExecutedOnReceiver: otherCount === 0,
  });
}

async function sessions(env: Env): Promise<Response> {
  const nullSession = invoke(env.DB.withSession, env.DB, [null]) as D1DatabaseSession;
  const blankSession = env.DB.withSession("   ");
  const trimmedSession = env.DB.withSession(" token ");
  const primary = env.DB.withSession("first-primary");
  const before = primary.getBookmark();
  const count = await primary.prepare("SELECT count(*) AS n FROM portable").first<number>("n");
  const bookmark = primary.getBookmark();
  const resumed = env.DB.withSession(bookmark ?? "missing");
  const resumedBefore = resumed.getBookmark();
  const resumedCount = await resumed.prepare("SELECT count(*) AS n FROM portable").first<number>("n");
  return Response.json({
    defaultsNull: nullSession.getBookmark() === null && blankSession.getBookmark() === null,
    trimmed: trimmedSession.getBookmark() === "token",
    before,
    count,
    bookmark: typeof bookmark === "string" && bookmark.length > 0,
    resumedBefore: resumedBefore === bookmark,
    resumedCount,
    resumedAfter: typeof resumed.getBookmark() === "string" && resumed.getBookmark()!.length > 0,
  });
}

async function errors(env: Env): Promise<Response> {
  const prepared = env.DB.prepare("SELECT value FROM portable ORDER BY id");
  const observed = {
    emptyBatch: await capture(() => env.DB.batch([])),
    invalidBatch: await capture(() => invoke(env.DB.batch, env.DB, [[{}]])),
    bindUndefined: await capture(() => invoke(prepared.bind, prepared, [undefined])),
    bindObject: await capture(() => invoke(prepared.bind, prepared, [{}])),
    missingColumn: await capture(() => prepared.first("missing")),
    numberedColumn: await capture(() => invoke(prepared.first, prepared, [42])),
    emptyAll: await capture(() => env.DB.prepare("").all()),
    emptyExec: await capture(() => env.DB.exec("   ")),
    dump: await capture(() => env.DB.dump()),
  };
  const permissive = {
    firstExtra: await invoke(prepared.first, prepared, ["value", "ignored"]),
    rawNull: await invoke(prepared.raw, prepared, [null]),
    rawExtra: await invoke(prepared.raw, prepared, [{ extra: true }]),
    nan: await env.DB.prepare("SELECT ? AS value").bind(Number.NaN).first("value"),
    infinity: await env.DB.prepare("SELECT ? AS value").bind(Number.POSITIVE_INFINITY).first("value"),
    unsafe: await env.DB.prepare("SELECT ? AS value").bind(Number.MAX_SAFE_INTEGER + 1).first("value"),
    dataView: await env.DB.prepare("SELECT length(?) AS value")
      .bind(new DataView(new ArrayBuffer(2))).first("value"),
  };
  return Response.json({ observed, permissive });
}

async function transaction(env: Env): Promise<Response> {
  let prefixed = false;
  try {
    await env.DB.batch([
      env.DB.prepare("INSERT INTO portable(value) VALUES ('rollback')"),
      env.DB.prepare("INSERT INTO portable(value) VALUES ('one')"),
    ]);
  } catch (error) {
    prefixed = error instanceof Error && error.message.startsWith("D1_ERROR:");
  }
  const count = await env.DB.prepare("SELECT count(*) AS n FROM portable WHERE value='rollback'").first<number>("n");
  return Response.json({ rolledBack: count === 0, prefixed });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/reset") return reset(env);
    if (path === "/surface") return surface(env);
    if (path === "/sessions") return sessions(env);
    if (path === "/errors") return errors(env);
    if (path === "/transaction") return transaction(env);
    if (path === "/cleanup") {
      await env.DB.exec("DROP TABLE portable");
      await env.OTHER.exec("DROP TABLE portable");
      return Response.json({ cleaned: true });
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;

interface Env {
  DB: D1Database;
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const prepared: D1PreparedStatement = env.DB.prepare("SELECT 1 AS value, 2 AS value");
    const bound: D1PreparedStatement = prepared.bind(null, true, 1, 1.5, "text", new Uint8Array(0));
    const run: D1Result<Record<string, unknown>> = await bound.run();
    const all: D1Result<{ value: number }> = await bound.all<{ value: number }>();
    const firstRow: Record<string, unknown> | null = await bound.first();
    const firstCol: number | null = await bound.first<number>("value");
    const rawRows: unknown[][] = await bound.raw();
    const rawNamed: [string[], ...unknown[][]] = await bound.raw({ columnNames: true });
    const rawPlain: unknown[][] = await bound.raw({ columnNames: false });
    const exec: D1ExecResult = await env.DB.exec("SELECT 1");
    const session: D1DatabaseSession = env.DB.withSession();
    const primary: D1DatabaseSession = env.DB.withSession("first-primary");
    const unconstrained: D1DatabaseSession = env.DB.withSession("first-unconstrained");
    const bookmark: D1SessionBookmark | null = session.getBookmark();
    const resumed: D1DatabaseSession = env.DB.withSession(bookmark ?? "opaque");
    const sessionPrepared: D1PreparedStatement = resumed.prepare("SELECT 1");
    const batched: D1Result<unknown>[] = await env.DB.batch([bound, sessionPrepared]);
    const sessionBatch: D1Result<unknown>[] = await session.batch([session.prepare("SELECT 1")]);
    const dump: Promise<ArrayBuffer> = env.DB.dump();
    const meta: D1Meta = run.meta;
    const duration: number = meta.duration;
    const sizeAfter: number = meta.size_after;
    const rowsRead: number = meta.rows_read;
    const rowsWritten: number = meta.rows_written;
    const lastRowId: number = meta.last_row_id;
    const changedDb: boolean = meta.changed_db;
    const changes: number = meta.changes;
    const region: string | undefined = meta.served_by_region;
    const colo: string | undefined = meta.served_by_colo;
    const primaryFlag: boolean | undefined = meta.served_by_primary;
    const sqlMs: number | undefined = meta.timings?.sql_duration_ms;
    const attempts: number | undefined = meta.total_attempts;
    const success: true = run.success;
    const results: unknown[] = all.results;
    const count: number = exec.count;
    const execDuration: number = exec.duration;
    const columns: string[] = rawNamed[0];

    // @ts-expect-error D1 success responses must not carry an error DTO
    const _errorDto: D1Result = { success: true, results: [], meta, error: "nope" };
    // @ts-expect-error first(column) requires a string column name
    await bound.first(1);
    // @ts-expect-error dump() is not present on D1DatabaseSession
    await session.dump();
    // @ts-expect-error exec() is not present on D1DatabaseSession
    await session.exec("SELECT 1");
    // @ts-expect-error served_by is not a D1Meta field
    const _servedBy: string = meta.served_by;

    return new Response(JSON.stringify({
      duration, sizeAfter, rowsRead, rowsWritten, lastRowId, changedDb, changes,
      region, colo, primaryFlag, sqlMs, attempts, success, resultCount: results.length,
      count, execDuration, columns, firstRow, firstCol, rawRows: rawRows.length,
      rawPlain: rawPlain.length, batched: batched.length, sessionBatch: sessionBatch.length,
      dump: typeof dump.then, bookmark, primary: typeof primary.prepare,
      unconstrained: typeof unconstrained.prepare,
    }));
  },
} satisfies ExportedHandler<Env>;

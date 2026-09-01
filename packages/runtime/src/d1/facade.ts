import type { D1QueryMode, D1RawTransport, D1SessionWire, D1StatementDto, D1Value } from "./protocol.js";

interface DatabaseState { raw: D1RawTransport }
interface SessionState {
  raw: D1RawTransport;
  constraint: "first-primary" | "first-unconstrained" | "bookmark";
  pendingBookmark: string | null;
  observedBookmark: string | null;
  observedVersion: number | null;
}
interface StatementState extends D1StatementDto { owner: object }
interface QueryResult {
  columns: string[];
  rows: (null | string | number | number[])[][];
  meta: Record<string, unknown>;
}
const databaseState = new WeakMap<object, DatabaseState>();
const sessionState = new WeakMap<object, SessionState>();
const statementState = new WeakMap<object, StatementState>();
const CORE_META = ["duration", "size_after", "rows_read", "rows_written", "last_row_id", "changed_db", "changes"];
const OPTIONAL_META = ["served_by_region", "served_by_colo", "served_by_primary", "timings", "total_attempts"];

function typeError(code: string): never {
  throw new TypeError(code);
}

function publicError(prefix: "D1_ERROR" | "D1_EXEC_ERROR" | "D1_TYPE_ERROR", message: string): never {
  throw new Error(`${prefix}: ${message}`, { cause: new Error(message) });
}

function failureCode(error: unknown): string {
  if (error !== null && (typeof error === "object" || typeof error === "function")) {
    for (const key of ["stableCode", "message"]) {
      const descriptor = Object.getOwnPropertyDescriptor(error, key);
      if (descriptor && "value" in descriptor && typeof descriptor.value === "string"
          && /^[A-Z][A-Z0-9_]{1,127}$/.test(descriptor.value)) return descriptor.value;
    }
  }
  return "Something went wrong";
}

function queryError(error: unknown): never {
  if (error instanceof Error && error.message.startsWith("D1_ERROR:")) throw error;
  if (error instanceof TypeError && error.message === "D1_INTERNAL_PROTOCOL_ERROR") throw error;
  publicError("D1_ERROR", failureCode(error));
}

function execError(error: unknown): never {
  if (error instanceof Error && error.message.startsWith("D1_EXEC_ERROR:")) throw error;
  publicError("D1_EXEC_ERROR", failureCode(error));
}

function bindTypeError(value: unknown): never {
  publicError("D1_TYPE_ERROR", `Type '${typeof value}' not supported for value '${value}'`);
}

function arrayLikeView(value: ArrayBufferView): value is ArrayBufferView & ArrayLike<unknown> {
  return typeof Reflect.get(value, "length") === "number";
}

function normalizeValue(value: unknown): D1Value {
  if (value === null) return null;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    if (value.every(item => typeof item === "number" && item >= 0 && item < 256)) {
      return Uint8Array.from(value);
    }
    bindTypeError(value);
  }
  if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
  if (ArrayBuffer.isView(value)) {
    const items = arrayLikeView(value) ? Array.from(value) : [];
    if (items.every((item): item is number => typeof item === "number" && item >= 0 && item < 256)) {
      return Uint8Array.from(items);
    }
    return items.join(",");
  }
  bindTypeError(value);
}

function databaseOwner(value: object): DatabaseState {
  const state = databaseState.get(value);
  if (state) return state;
  const session = sessionState.get(value);
  if (session) return { raw: session.raw };
  typeError("D1_INTERNAL_PROTOCOL_ERROR");
}

function statement(owner: object, sql: string, params: D1Value[] = []): D1PreparedStatement {
  const result = new D1PreparedStatement();
  statementState.set(result, Object.freeze({ owner, sql, params: Object.freeze(params) }));
  return result;
}

function assertStatement(value: unknown): StatementState {
  if (value === null || (typeof value !== "object" && typeof value !== "function")) typeError("D1_INVALID_BATCH");
  const state = statementState.get(value);
  if (!state) typeError("D1_INVALID_BATCH");
  return state;
}

function dto(state: StatementState): D1StatementDto {
  return { sql: state.sql, params: state.params.slice() };
}

function transportFor(owner: object): D1RawTransport {
  return databaseOwner(owner).raw;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function finiteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function assertMeta(meta: unknown): Record<string, unknown> {
  if (!isRecord(meta)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  const allowed = new Set([...CORE_META, ...OPTIONAL_META]);
  if (Object.keys(meta).some((key) => !allowed.has(key))
      || CORE_META.some((key) => !Object.prototype.hasOwnProperty.call(meta, key))
      || typeof meta.changed_db !== "boolean"
      || CORE_META.filter((key) => key !== "changed_db").some((key) => !finiteNumber(meta[key]))) {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  if (meta.served_by_region !== undefined && typeof meta.served_by_region !== "string") {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  if (meta.served_by_colo !== undefined && typeof meta.served_by_colo !== "string") {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  if (meta.served_by_primary !== undefined && typeof meta.served_by_primary !== "boolean") {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  if (meta.total_attempts !== undefined && !finiteNumber(meta.total_attempts)) {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  if (meta.timings !== undefined) {
    if (!isRecord(meta.timings)
        || Object.keys(meta.timings).some((key) => key !== "sql_duration_ms")
        || !finiteNumber(meta.timings.sql_duration_ms)) {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
  }
  return meta;
}

function outputValue(value: unknown): null | string | number | number[] {
  if (value === null || typeof value === "string"
      || (typeof value === "number" && Number.isFinite(value))) return value;
  if (value instanceof Uint8Array) return Array.from(value);
  if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
  typeError("D1_INTERNAL_PROTOCOL_ERROR");
}

function assertQueryResponse(result: unknown, session: boolean): { results: QueryResult[]; bookmark: string | null; stateVersion: number } {
  if (!isRecord(result) || !Array.isArray(result.results) || !Number.isSafeInteger(result.stateVersion)
      || (result.stateVersion as number) < 0) {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  const results = result.results.map((entry: unknown) => {
    if (!isRecord(entry) || !Array.isArray(entry.columns)
        || !Array.isArray(entry.rows) || entry.columns.length > 100
        || entry.columns.some((name) => typeof name !== "string")) {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
    const columns = entry.columns as string[];
    const rows = entry.rows.map((row: unknown) => {
      if (!Array.isArray(row) || row.length !== columns.length) {
        typeError("D1_INTERNAL_PROTOCOL_ERROR");
      }
      return row.map(outputValue);
    });
    return { columns: columns.slice(), rows, meta: assertMeta(entry.meta) };
  });
  if (session) {
    if (typeof result.bookmark !== "string" || result.bookmark.length === 0) {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
  } else if (result.bookmark != null && result.bookmark !== "") {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  return {
    results,
    bookmark: session ? result.bookmark as string : null,
    stateVersion: result.stateVersion as number,
  };
}

function objectRow(columns: readonly string[], values: QueryResult["rows"][number]): Record<string, null | string | number | number[]> {
  const output: Record<string, null | string | number | number[]> = {};
  for (let index = 0; index < columns.length; index++) {
    Object.defineProperty(output, columns[index]!, {
      value: values[index], enumerable: true, configurable: true, writable: true,
    });
  }
  return output;
}

function d1Result(entry: QueryResult) {
  return {
    success: true,
    results: entry.rows.map((row) => objectRow(entry.columns, row)),
    meta: entry.meta,
  };
}

function sessionWire(owner: object): D1SessionWire {
  const session = sessionState.get(owner);
  if (!session) return { kind: 0 };
  if (session.observedBookmark) return { kind: 3, bookmark: session.observedBookmark };
  if (session.constraint === "bookmark" && session.pendingBookmark) {
    return { kind: 3, bookmark: session.pendingBookmark };
  }
  return { kind: session.constraint === "first-primary" ? 2 : 1 };
}

function observeBookmark(owner: object, bookmark: string | null, stateVersion: number) {
  const session = sessionState.get(owner);
  if (!session || bookmark === null) return;
  if (session.observedVersion === null || stateVersion > session.observedVersion) {
    session.observedBookmark = bookmark;
    session.observedVersion = stateVersion;
  }
}

async function terminal(state: StatementState, mode: D1QueryMode): Promise<QueryResult> {
  try {
    if (typeof state.sql !== "string") {
      publicError("D1_ERROR", "Malformed input: expected a SQL string");
    }
    if (state.sql.trim().length === 0) publicError("D1_ERROR", "No SQL statements detected.");
    const session = sessionState.has(state.owner);
    const response = await transportFor(state.owner).query(mode, [dto(state)], sessionWire(state.owner));
    const decoded = assertQueryResponse(response, session);
    if (decoded.results.length !== 1) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    observeBookmark(state.owner, decoded.bookmark, decoded.stateVersion);
    return decoded.results[0]!;
  } catch (error) {
    queryError(error);
  }
}

function batchOwner(owner: object, values: unknown): StatementState[] {
  if (!Array.isArray(values)) {
    publicError("D1_ERROR", "Malformed input: expected an array of prepared statements");
  }
  if (values.length === 0) publicError("D1_ERROR", "No SQL statements detected.");
  if (values.length > 0xffff) publicError("D1_ERROR", "D1_LIMIT_ERROR");
  try {
    return values.map(assertStatement);
  } catch {
    let input = "[invalid prepared statement]";
    try { input = JSON.stringify(values); } catch { /* use the bounded fallback */ }
    publicError(
      "D1_ERROR",
      `Malformed input: ${input.slice(0, 512)}, should be {sql: string, params?: any[]} or an array of these query objects`,
    );
  }
}

async function runBatch(owner: object, values: unknown) {
  try {
    const states = batchOwner(owner, values);
    const session = sessionState.has(owner);
    const response = await transportFor(owner).query("batch", states.map(dto), sessionWire(owner));
    const decoded = assertQueryResponse(response, session);
    if (decoded.results.length !== states.length) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    observeBookmark(owner, decoded.bookmark, decoded.stateVersion);
    return decoded.results.map(d1Result);
  } catch (error) {
    queryError(error);
  }
}

export class D1PreparedStatement {
  bind(...values: unknown[]) {
    const state = assertStatement(this);
    return statement(state.owner, state.sql, values.map(normalizeValue));
  }

  async run() {
    return d1Result(await terminal(assertStatement(this), "run"));
  }

  async all() {
    return d1Result(await terminal(assertStatement(this), "all"));
  }

  async first(columnName?: string) {
    const entry = await terminal(assertStatement(this), "all");
    if (entry.rows.length === 0) return null;
    const row = objectRow(entry.columns, entry.rows[0]!);
    if (columnName === undefined) return row;
    const value = row[columnName];
    if (value === undefined) {
      throw new Error(`D1_COLUMN_NOTFOUND: Column not found (${columnName})`, {
        cause: new Error("Column not found"),
      });
    }
    return value;
  }

  async raw(options?: { columnNames?: boolean } | null) {
    const entry = await terminal(assertStatement(this), "raw");
    const rows = entry.rows.map((row) => row.slice());
    if (options?.columnNames) rows.unshift(entry.columns.slice());
    return rows;
  }
}

export class D1DatabaseSession {
  prepare(sql: string) {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return statement(this, sql);
  }

  batch(statements: readonly D1PreparedStatement[]) {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return runBatch(this, statements);
  }

  getBookmark() {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    const state = sessionState.get(this)!;
    return state.observedBookmark ?? (state.constraint === "bookmark" ? state.pendingBookmark : null);
  }
}

export class D1Database {
  constructor(raw: unknown) {
    if (!rawTransport(raw)) {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
    databaseState.set(this, Object.freeze({ raw }));
  }

  prepare(sql: string) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return statement(this, sql);
  }

  batch(statements: readonly D1PreparedStatement[]) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return runBatch(this, statements);
  }

  async exec(sql: string) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    const trimmed = sql.trim();
    if (trimmed.length === 0) {
      throw new Error("D1_EXEC_ERROR: Error in line 1: : No SQL statements detected.", {
        cause: new Error("Error in line 1: : No SQL statements detected."),
      });
    }
    try {
      const result = await databaseOwner(this).raw.exec(sql, {});
      if (!isRecord(result) || typeof result.count !== "number" || !Number.isSafeInteger(result.count)
          || result.count < 1 || typeof result.duration !== "number"
          || !Number.isFinite(result.duration)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
      return { count: result.count, duration: result.duration };
    } catch (error) {
      execError(error);
    }
  }

  withSession(constraintOrBookmark?: string) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    const normalized = constraintOrBookmark?.trim();
    let constraint: SessionState["constraint"] = "first-unconstrained";
    let pendingBookmark: string | null = null;
    if (normalized) {
      if (normalized !== "first-unconstrained") {
        if (normalized === "first-primary") constraint = "first-primary";
        else {
          constraint = "bookmark";
          pendingBookmark = normalized;
        }
      }
    }
    const session = new D1DatabaseSession();
    sessionState.set(session, {
      raw: databaseOwner(this).raw,
      constraint,
      pendingBookmark,
      observedBookmark: null,
      observedVersion: null,
    });
    return session;
  }

  async dump(): Promise<ArrayBuffer> {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    throw new Error("D1_DUMP_ERROR: Status + 400", { cause: new Error("Status 400") });
  }
}

function rawTransport(raw: unknown): raw is D1RawTransport {
  return isRecord(raw) && typeof raw.query === "function" && typeof raw.exec === "function";
}

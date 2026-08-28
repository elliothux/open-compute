import type { D1QueryMode, D1RawTransport, D1StatementDto, D1Value } from "./protocol.js";

interface DatabaseState { raw: D1RawTransport }
interface StatementState extends D1StatementDto { owner: object }
interface QueryResult {
  columns: string[];
  rows: (null | string | number | number[])[][];
  meta: Record<string, unknown>;
}
const databaseState = new WeakMap<object, DatabaseState>();
const sessionState = new WeakMap<object, DatabaseState & { root: D1Database; constraint: string }>();
const statementState = new WeakMap<object, StatementState>();
const encoder = new TextEncoder();

function typeError(code: string): never {
  throw new TypeError(code);
}

function sqlText(value: unknown): string {
  if (typeof value !== "string" || value.trim().length === 0) typeError("D1_SQL_INVALID");
  if (encoder.encode(value).byteLength > 100000) typeError("D1_SQL_INVALID");
  return value;
}

function normalizeValue(value: unknown): D1Value {
  if (value === null) return null;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (typeof value === "number") {
    if (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value))) {
      typeError("D1_TYPE_ERROR");
    }
    return value;
  }
  if (typeof value === "string") return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
  if (ArrayBuffer.isView(value)) {
    const out = new Uint8Array(value.byteLength);
    out.set(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
    return out;
  }
  typeError("D1_TYPE_ERROR");
}

function databaseOwner(value: object): DatabaseState {
  const state = databaseState.get(value) ?? sessionState.get(value);
  if (state) return state;
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

function assertMeta(meta: unknown): Record<string, unknown> {
  if (!isRecord(meta)) {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
  }
  const required = [
    "served_by", "served_by_primary", "duration", "changes", "last_row_id",
    "changed_db", "size_after", "rows_read", "rows_written",
  ];
  if (Object.keys(meta).some((key) => !required.includes(key))
      || required.some((key) => !Object.prototype.hasOwnProperty.call(meta, key))
      || meta.served_by !== "open-compute-local"
      || meta.served_by_primary !== true
      || typeof meta.changed_db !== "boolean"
      || required.slice(2).filter((key) => key !== "changed_db")
        .some((key) => typeof meta[key] !== "number" || !Number.isFinite(meta[key]))) {
    typeError("D1_INTERNAL_PROTOCOL_ERROR");
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

function assertTransportResult(result: unknown): QueryResult[] {
  if (!isRecord(result)
      || !Array.isArray(result.results)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  return result.results.map((entry: unknown) => {
    if (!isRecord(entry) || !Array.isArray(entry.columns)
        || !Array.isArray(entry.rows) || entry.columns.length > 100
        || entry.columns.some((name) => typeof name !== "string")) {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
    const columns = entry.columns as string[]; // Every column was checked above.
    const rows = entry.rows.map((row: unknown) => {
      if (!Array.isArray(row) || row.length !== columns.length) {
        typeError("D1_INTERNAL_PROTOCOL_ERROR");
      }
      return row.map(outputValue);
    });
    return { columns: columns.slice(), rows, meta: assertMeta(entry.meta) };
  });
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

async function terminal(state: StatementState, mode: D1QueryMode): Promise<QueryResult> {
  const response = await transportFor(state.owner).query(mode, [dto(state)], {});
  const results = assertTransportResult(response);
  if (results.length !== 1) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  return results[0]!;
}

function batchOwner(owner: object, values: unknown): StatementState[] {
  if (!Array.isArray(values) || values.length < 1 || values.length > 100) {
    typeError("D1_INVALID_BATCH");
  }
  const states = values.map(assertStatement);
  if (states.some((state) => state.owner !== owner)) typeError("D1_INVALID_BATCH");
  return states;
}

async function runBatch(owner: object, values: unknown) {
  const states = batchOwner(owner, values);
  const response = await transportFor(owner).query("batch", states.map(dto), {});
  const results = assertTransportResult(response);
  if (results.length !== states.length) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  return results.map(d1Result);
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
    if (arguments.length > 1 || (arguments.length === 1 && typeof columnName !== "string")) {
      typeError("D1_TYPE_ERROR");
    }
    const entry = await terminal(assertStatement(this), "all");
    if (entry.rows.length === 0) return null;
    if (arguments.length === 0) return objectRow(entry.columns, entry.rows[0]!);
    const index = entry.columns.lastIndexOf(columnName!);
    if (index < 0) typeError("D1_COLUMN_NOTFOUND");
    return entry.rows[0]![index];
  }

  async raw(options: unknown = {}) {
    if (!isRecord(options)
        || Object.keys(options).some((key) => key !== "columnNames")
        || (options.columnNames !== undefined && typeof options.columnNames !== "boolean")) {
      typeError("D1_TYPE_ERROR");
    }
    const entry = await terminal(assertStatement(this), "raw");
    const rows = entry.rows.map((row) => row.slice());
    if (options.columnNames === true) rows.unshift(entry.columns.slice());
    return rows;
  }
}

export class D1DatabaseSession {
  prepare(sql: string) {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return statement(this, sqlText(sql));
  }

  batch(statements: readonly D1PreparedStatement[]) {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return runBatch(this, statements);
  }

  getBookmark() {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return null;
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
    return statement(this, sqlText(sql));
  }

  batch(statements: readonly D1PreparedStatement[]) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return runBatch(this, statements);
  }

  async exec(sql: string) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    const result = await databaseOwner(this).raw.exec(sqlText(sql), {});
    if (!isRecord(result) || typeof result.count !== "number" || !Number.isSafeInteger(result.count)
        || result.count < 1 || typeof result.duration !== "number"
        || !Number.isFinite(result.duration)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return { count: result.count, duration: result.duration };
  }

  withSession(constraint = "first-unconstrained") {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    if (constraint !== "first-primary" && constraint !== "first-unconstrained") {
      typeError("D1_SESSION_UNSUPPORTED");
    }
    const session = new D1DatabaseSession();
    sessionState.set(session, Object.freeze({ raw: databaseOwner(this).raw, root: this, constraint }));
    return session;
  }
}

function rawTransport(raw: unknown): raw is D1RawTransport {
  return isRecord(raw) && typeof raw.query === "function" && typeof raw.exec === "function";
}

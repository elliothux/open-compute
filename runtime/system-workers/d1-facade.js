const databaseState = new WeakMap();
const sessionState = new WeakMap();
const statementState = new WeakMap();
const encoder = new TextEncoder();

function typeError(code) {
  throw new TypeError(code);
}

function sqlText(value) {
  if (typeof value !== "string" || value.trim().length === 0) typeError("D1_SQL_INVALID");
  if (encoder.encode(value).byteLength > 100000) typeError("D1_SQL_INVALID");
  return value;
}

function normalizeValue(value) {
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

function databaseOwner(value) {
  if (databaseState.has(value)) return databaseState.get(value);
  if (sessionState.has(value)) return sessionState.get(value);
  typeError("D1_INTERNAL_PROTOCOL_ERROR");
}

function statement(owner, sql, params = []) {
  const result = Object.create(D1PreparedStatement.prototype);
  statementState.set(result, Object.freeze({ owner, sql, params: Object.freeze(params) }));
  return result;
}

function assertStatement(value) {
  const state = statementState.get(value);
  if (!state) typeError("D1_INVALID_BATCH");
  return state;
}

function dto(state) {
  return { sql: state.sql, params: state.params.slice() };
}

function transportFor(owner) {
  return databaseOwner(owner).raw;
}

function assertMeta(meta) {
  if (!meta || typeof meta !== "object" || Array.isArray(meta)) {
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

function outputValue(value) {
  if (value === null || typeof value === "string"
      || (typeof value === "number" && Number.isFinite(value))) return value;
  if (value instanceof Uint8Array) return Array.from(value);
  if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
  typeError("D1_INTERNAL_PROTOCOL_ERROR");
}

function assertTransportResult(result) {
  if (!result || typeof result !== "object" || Array.isArray(result)
      || !Array.isArray(result.results)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  return result.results.map((entry) => {
    if (!entry || typeof entry !== "object" || !Array.isArray(entry.columns)
        || !Array.isArray(entry.rows) || entry.columns.length > 100
        || entry.columns.some((name) => typeof name !== "string")) {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
    const rows = entry.rows.map((row) => {
      if (!Array.isArray(row) || row.length !== entry.columns.length) {
        typeError("D1_INTERNAL_PROTOCOL_ERROR");
      }
      return row.map(outputValue);
    });
    return { columns: entry.columns.slice(), rows, meta: assertMeta(entry.meta) };
  });
}

function objectRow(columns, values) {
  const output = {};
  for (let index = 0; index < columns.length; index++) {
    Object.defineProperty(output, columns[index], {
      value: values[index], enumerable: true, configurable: true, writable: true,
    });
  }
  return output;
}

function d1Result(entry) {
  return {
    success: true,
    results: entry.rows.map((row) => objectRow(entry.columns, row)),
    meta: entry.meta,
  };
}

async function terminal(state, mode) {
  const response = await transportFor(state.owner).query(mode, [dto(state)], {});
  const results = assertTransportResult(response);
  if (results.length !== 1) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  return results[0];
}

function batchOwner(owner, values) {
  if (!Array.isArray(values) || values.length < 1 || values.length > 100) {
    typeError("D1_INVALID_BATCH");
  }
  const states = values.map(assertStatement);
  if (states.some((state) => state.owner !== owner)) typeError("D1_INVALID_BATCH");
  return states;
}

async function runBatch(owner, values) {
  const states = batchOwner(owner, values);
  const response = await transportFor(owner).query("batch", states.map(dto), {});
  const results = assertTransportResult(response);
  if (results.length !== states.length) typeError("D1_INTERNAL_PROTOCOL_ERROR");
  return results.map(d1Result);
}

export class D1PreparedStatement {
  bind(...values) {
    const state = assertStatement(this);
    return statement(state.owner, state.sql, values.map(normalizeValue));
  }

  async run() {
    return d1Result(await terminal(assertStatement(this), "run"));
  }

  async all() {
    return d1Result(await terminal(assertStatement(this), "all"));
  }

  async first(columnName) {
    if (arguments.length > 1 || (arguments.length === 1 && typeof columnName !== "string")) {
      typeError("D1_TYPE_ERROR");
    }
    const entry = await terminal(assertStatement(this), "all");
    if (entry.rows.length === 0) return null;
    if (arguments.length === 0) return objectRow(entry.columns, entry.rows[0]);
    const index = entry.columns.lastIndexOf(columnName);
    if (index < 0) typeError("D1_COLUMN_NOTFOUND");
    return entry.rows[0][index];
  }

  async raw(options = {}) {
    if (!options || typeof options !== "object" || Array.isArray(options)
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
  prepare(sql) {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return statement(this, sqlText(sql));
  }

  batch(statements) {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return runBatch(this, statements);
  }

  getBookmark() {
    if (!sessionState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return null;
  }
}

export class D1Database {
  constructor(raw) {
    if (!raw || typeof raw !== "object"
        || typeof raw.query !== "function" || typeof raw.exec !== "function") {
      typeError("D1_INTERNAL_PROTOCOL_ERROR");
    }
    databaseState.set(this, Object.freeze({ raw }));
  }

  prepare(sql) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return statement(this, sqlText(sql));
  }

  batch(statements) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return runBatch(this, statements);
  }

  async exec(sql) {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    const result = await databaseState.get(this).raw.exec(sqlText(sql), {});
    if (!result || typeof result !== "object" || !Number.isSafeInteger(result.count)
        || result.count < 1 || typeof result.duration !== "number"
        || !Number.isFinite(result.duration)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    return { count: result.count, duration: result.duration };
  }

  withSession(constraint = "first-unconstrained") {
    if (!databaseState.has(this)) typeError("D1_INTERNAL_PROTOCOL_ERROR");
    if (constraint !== "first-primary" && constraint !== "first-unconstrained") {
      typeError("D1_SESSION_UNSUPPORTED");
    }
    const session = Object.create(D1DatabaseSession.prototype);
    sessionState.set(session, Object.freeze({ raw: databaseState.get(this).raw, root: this, constraint }));
    return session;
  }
}

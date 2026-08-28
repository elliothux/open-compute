import type { AlarmIndexCapability, AlarmProjection } from "./protocol.js";

interface AlarmRow extends AlarmProjection { id: 1; inFlight: boolean; lastErrorCode: string | null; updatedAtMs: number }
interface AlarmTransaction { sync: boolean; mutated: boolean }
interface PreparedAlarm { context: DurableObjectState; storage: DurableObjectStorage; index: AlarmIndexCapability }
type AlarmSqlRow = Record<string, SqlStorageValue> & { row_token: string; last_error_code: string | null };

const TABLE = "__open_compute_do_alarm";
const ROW_TOKEN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const INTERNAL_METHOD = /^__openCompute/;

function alarmFailure(code: string, type: ErrorConstructor = Error) {
  const error = Object.assign(new type(code), { stableCode: code });
  error.stack = `${error.name}: ${code}`;
  return error;
}

function ensureTable(storage: DurableObjectStorage) {
  storage.sql.exec(`
    CREATE TABLE IF NOT EXISTS ${TABLE} (
      id INTEGER PRIMARY KEY CHECK(id = 1),
      scheduled_time_ms INTEGER NOT NULL CHECK(scheduled_time_ms > 0),
      retry_count INTEGER NOT NULL DEFAULT 0 CHECK(retry_count BETWEEN 0 AND 6),
      in_flight INTEGER NOT NULL DEFAULT 0 CHECK(in_flight IN (0, 1)),
      row_token TEXT NOT NULL,
      last_error_code TEXT,
      updated_at_ms INTEGER NOT NULL
    ) STRICT
  `);
}

function validRow(row: Record<string, SqlStorageValue> | undefined): row is AlarmSqlRow {
  return row !== undefined
    && Number(row.id) === 1
    && Number.isSafeInteger(Number(row.scheduled_time_ms))
    && Number(row.scheduled_time_ms) > 0
    && Number.isSafeInteger(Number(row.retry_count))
    && Number(row.retry_count) >= 0
    && Number(row.retry_count) <= 6
    && (Number(row.in_flight) === 0 || Number(row.in_flight) === 1)
    && typeof row.row_token === "string"
    && ROW_TOKEN.test(row.row_token)
    && (row.last_error_code === null || typeof row.last_error_code === "string")
    && Number.isSafeInteger(Number(row.updated_at_ms));
}

function readRow(storage: DurableObjectStorage): AlarmRow | null {
  ensureTable(storage);
  const rows = storage.sql.exec(
    `SELECT id, scheduled_time_ms, retry_count, in_flight, row_token,
            last_error_code, updated_at_ms FROM ${TABLE} WHERE id = 1`,
  ).toArray();
  if (!rows.length) return null;
  const row = rows[0];
  if (!validRow(row)) {
    storage.sql.exec(`DELETE FROM ${TABLE} WHERE id = 1`);
    return null;
  }
  return {
    id: 1,
    scheduledTimeMs: Number(row.scheduled_time_ms),
    retryCount: Number(row.retry_count),
    inFlight: Number(row.in_flight) === 1,
    rowToken: row.row_token,
    lastErrorCode: row.last_error_code,
    updatedAtMs: Number(row.updated_at_ms),
  };
}

function exactDelete(storage: DurableObjectStorage, rowToken: string) {
  ensureTable(storage);
  storage.sql.exec(`DELETE FROM ${TABLE} WHERE id = 1 AND row_token = ?`, rowToken);
}

function restoreIfAbsent(storage: DurableObjectStorage, row: AlarmRow | null) {
  if (!row) return;
  ensureTable(storage);
  storage.sql.exec(
    `INSERT INTO ${TABLE}
       (id, scheduled_time_ms, retry_count, in_flight, row_token, last_error_code, updated_at_ms)
     VALUES (1, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING`,
    row.scheduledTimeMs,
    row.retryCount,
    row.inFlight ? 1 : 0,
    row.rowToken,
    row.lastErrorCode,
    row.updatedAtMs,
  );
}

function alarmTime(value: unknown): number {
  const raw = value instanceof Date ? value.getTime() : value;
  if (typeof raw !== "number" || !Number.isFinite(raw) || !Number.isSafeInteger(raw) || raw <= 0) {
    throw alarmFailure("DO_ALARM_TIME_INVALID", TypeError);
  }
  return raw;
}

function projection(row: AlarmRow): AlarmProjection {
  return {
    scheduledTimeMs: row.scheduledTimeMs,
    retryCount: row.retryCount,
    rowToken: row.rowToken,
  };
}

function setAlarm(storage: DurableObjectStorage, index: AlarmIndexCapability, transaction: AlarmTransaction | null, value: unknown) {
  if (transaction?.sync) {
    throw new TypeError("setAlarm() is not supported inside transactionSync()");
  }
  return (async () => {
    const scheduledTimeMs = alarmTime(value);
    const rowToken = crypto.randomUUID();
    ensureTable(storage);
    storage.sql.exec(
      `INSERT INTO ${TABLE}
         (id, scheduled_time_ms, retry_count, in_flight, row_token, last_error_code, updated_at_ms)
       VALUES (1, ?, 0, 0, ?, NULL, ?)
       ON CONFLICT(id) DO UPDATE SET scheduled_time_ms = excluded.scheduled_time_ms,
         retry_count = 0, in_flight = 0, row_token = excluded.row_token,
         last_error_code = NULL, updated_at_ms = excluded.updated_at_ms`,
      scheduledTimeMs,
      rowToken,
      Date.now(),
    );
    if (transaction) {
      transaction.mutated = true;
      return;
    }
    try {
      await index.upsert({ scheduledTimeMs, retryCount: 0, rowToken });
    } catch {
      exactDelete(storage, rowToken);
      throw alarmFailure("DO_ALARM_INDEX_UNAVAILABLE");
    }
  })();
}

function getAlarm(storage: DurableObjectStorage, index: AlarmIndexCapability, transaction: AlarmTransaction | null) {
  return (async () => {
    const row = readRow(storage);
    if (!row || row.inFlight) return null;
    if (!transaction) {
      try {
        await index.upsert(projection(row));
      } catch {
        // The object-local row remains authority; later activation/scan repair retries.
      }
    }
    return row.scheduledTimeMs;
  })();
}

function deleteAlarm(storage: DurableObjectStorage, index: AlarmIndexCapability, transaction: AlarmTransaction | null) {
  if (transaction?.sync) {
    throw new TypeError("deleteAlarm() is not supported inside transactionSync()");
  }
  return (async () => {
    const row = readRow(storage);
    if (!row) return;
    exactDelete(storage, row.rowToken);
    if (transaction) {
      transaction.mutated = true;
      return;
    }
    try {
      await index.delete(row.rowToken);
    } catch {
      restoreIfAbsent(storage, row);
      throw alarmFailure("DO_ALARM_INDEX_UNAVAILABLE");
    }
  })();
}

function quoteSqlIdentifier(name: string): string {
  return `"${String(name).replaceAll('"', '""')}"`;
}

async function deleteAllStorage(storage: DurableObjectStorage, transaction: DurableObjectTransaction) {
  const entries = await transaction.list();
  const keys = [...entries.keys()];
  if (keys.length) await transaction.delete(keys);
  storage.sql.exec("PRAGMA defer_foreign_keys = ON");
  const objects = [...storage.sql.exec(
    "SELECT type, name FROM sqlite_master "
      + "WHERE type IN ('trigger', 'view', 'table') "
      + "ORDER BY CASE type WHEN 'trigger' THEN 0 WHEN 'view' THEN 1 ELSE 2 END, rowid DESC",
  )];
  for (const object of objects) {
    const type = String(object.type);
    const name = String(object.name);
    const lower = name.toLowerCase();
    if (lower.startsWith("sqlite_") || lower.startsWith("_cf_")) continue;
    if (!["trigger", "view", "table"].includes(type)) continue;
    storage.sql.exec(`DROP ${type.toUpperCase()} IF EXISTS ${quoteSqlIdentifier(name)}`);
  }
}

async function flushTransaction(rootStorage: DurableObjectStorage, index: AlarmIndexCapability, initial: AlarmRow | null, final: AlarmRow | null) {
  try {
    if (final) {
      await index.upsert(projection(final));
    }
    else if (initial) await index.delete(initial.rowToken);
  } catch {
    if (final) exactDelete(rootStorage, final.rowToken);
    else restoreIfAbsent(rootStorage, initial);
    throw alarmFailure("DO_ALARM_INDEX_UNAVAILABLE");
  }
}

function sameProjection(left: AlarmRow | null, right: AlarmRow | null): boolean {
  if (!left || !right) return left === right;
  return left.rowToken === right.rowToken
    && left.scheduledTimeMs === right.scheduledTimeMs
    && left.retryCount === right.retryCount;
}

function wrapStorage<T extends DurableObjectStorage | DurableObjectTransaction>(storage: T, index: AlarmIndexCapability,
  transaction: AlarmTransaction | null, rootStorage: DurableObjectStorage): T {
  return new Proxy(storage, {
    get(target, property) {
      const alarmStorage = rootStorage;
      if (property === "setAlarm") return (value: unknown) => setAlarm(alarmStorage, index, transaction, value);
      if (property === "getAlarm") return () => getAlarm(alarmStorage, index, transaction);
      if (property === "deleteAlarm") return () => deleteAlarm(alarmStorage, index, transaction);
      if (property === "transactionSync") {
        if (!("transactionSync" in target)) throw new TypeError("nested transactionSync() is unsupported");
        return (callback: (storage: T) => unknown) => target.transactionSync(() => callback(
          wrapStorage(target, index, { sync: true, mutated: false }, rootStorage),
        ));
      }
      if (property === "transaction") {
        if (!("transaction" in target)) throw new TypeError("nested transaction() is unsupported");
        return async (callback: (storage: DurableObjectTransaction) => unknown) => {
          const initial = readRow(target);
          const committed: { value?: { mutated: boolean; final: AlarmRow | null } } = {};
          const result = await target.transaction(async nativeTransaction => {
            const attempt = { sync: false, mutated: false };
            const value = await callback(
              wrapStorage(nativeTransaction, index, attempt, rootStorage),
            );
            committed.value = { mutated: attempt.mutated, final: readRow(rootStorage) };
            return value;
          });
          if (committed.value?.mutated && !sameProjection(initial, committed.value.final)) {
            await flushTransaction(rootStorage, index, initial, committed.value.final);
          }
          return result;
        };
      }
      if (property === "deleteAll") {
        if (!("transaction" in target)) throw new TypeError("deleteAll() inside a transaction is unsupported");
        return async (...args: unknown[]) => {
          if (args.length > 1) throw new TypeError("deleteAll() accepts at most one options argument");
          const row = readRow(target);
          await target.transaction(transaction => deleteAllStorage(target, transaction));
          if (row) {
            try {
              await index.delete(row.rowToken);
            } catch {
              throw alarmFailure("DO_ALARM_INDEX_UNAVAILABLE");
            }
          } else {
            try { await index.clear(); } catch { /* no alarm authority existed */ }
          }
        };
      }
      const value: unknown = Reflect.get(target, property, target);
      return typeof value === "function" ? (...args: unknown[]): unknown => Reflect.apply(value, target, args) : value;
    },
  });
}

/// Install the class-specific storage proxy before tenant construction.
export function prepareDurableObjectContext(ctx: DurableObjectState, index: AlarmIndexCapability): PreparedAlarm {
  if (!ctx?.storage || !index) throw alarmFailure("DO_ALARM_INDEX_UNAVAILABLE");
  ensureTable(ctx.storage);
  const storage = wrapStorage(ctx.storage, index, null, ctx.storage);
  let context = ctx;
  try {
    Object.defineProperty(ctx, "storage", { value: storage, configurable: true });
  } catch {
    context = new Proxy(ctx, {
      get(target, property) {
        if (property === "storage") return storage;
        const value: unknown = Reflect.get(target, property, target);
        return typeof value === "function" ? (...args: unknown[]): unknown => Reflect.apply(value, target, args) : value;
      },
    });
  }
  return Object.freeze({ context, storage, index });
}

/// Queue activation repair without exposing its private capability to tenant code.
export function activateDurableObjectAlarm(prepared: PreparedAlarm) {
  prepared.context.blockConcurrencyWhile(async () => {
    const row = readRow(prepared.storage);
    try {
      if (row) await prepared.index.upsert(projection(row));
      else await prepared.index.clear();
    } catch {
      // Repair is deliberately lower availability than ordinary DO dispatch.
    }
  });
}

/// Return the strict object-local DTO used by startup and periodic projection repair.
export async function repairDurableObjectAlarm(prepared: PreparedAlarm) {
  const row = readRow(prepared.storage);
  if (!row) return { exists: false };
  return {
    exists: true,
    scheduledTimeMs: row.scheduledTimeMs,
    retryCount: row.retryCount,
    rowToken: row.rowToken,
  };
}

/// Validate and deliver one private scheduler claim to the tenant alarm handler.
export async function dispatchDurableObjectAlarm(instance: unknown, handler: unknown, prepared: PreparedAlarm, payload: unknown) {
  if (payload === null || typeof payload !== "object" || !("rowToken" in payload) || !("retryCount" in payload)
      || typeof payload.rowToken !== "string" || !ROW_TOKEN.test(payload.rowToken)
      || typeof payload.retryCount !== "number" || !Number.isSafeInteger(payload.retryCount) || payload.retryCount < 0
      || payload.retryCount > 6) {
    throw alarmFailure("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
  }
  const row = readRow(prepared.storage);
  if (!row || row.rowToken !== payload.rowToken) return { outcome: "stale" };
  const now = Date.now();
  if (row.scheduledTimeMs > now) {
    return {
      outcome: "notDue",
      scheduledTimeMs: row.scheduledTimeMs,
      retryCount: row.retryCount,
    };
  }
  prepared.storage.sql.exec(
    `UPDATE ${TABLE} SET in_flight = 1, retry_count = ?, updated_at_ms = ?
     WHERE id = 1 AND row_token = ?`,
    payload.retryCount,
    now,
    payload.rowToken,
  );
  try {
    if (typeof handler === "function" && !INTERNAL_METHOD.test(handler.name || "")) {
      await Reflect.apply(handler, instance, [{
        retryCount: payload.retryCount,
        isRetry: payload.retryCount > 0,
      }]);
    }
    exactDelete(prepared.storage, payload.rowToken);
    return { outcome: "success" };
  } catch {
    const current = readRow(prepared.storage);
    if (!current || current.rowToken !== payload.rowToken) return { outcome: "stale" };
    if (payload.retryCount >= 6) {
      exactDelete(prepared.storage, payload.rowToken);
      return { outcome: "exhausted", errorCode: "DO_RUNTIME_EXCEPTION" };
    }
    const retryCount = payload.retryCount + 1;
    const scheduledTimeMs = Date.now() + 2000 * (2 ** payload.retryCount);
    prepared.storage.sql.exec(
      `UPDATE ${TABLE} SET scheduled_time_ms = ?, retry_count = ?, in_flight = 0,
              last_error_code = 'DO_RUNTIME_EXCEPTION', updated_at_ms = ?
       WHERE id = 1 AND row_token = ?`,
      scheduledTimeMs,
      retryCount,
      Date.now(),
      payload.rowToken,
    );
    const retry = { scheduledTimeMs, retryCount, rowToken: payload.rowToken };
    try { await prepared.index.upsert(retry); } catch { /* dispatcher response repairs it */ }
    return { outcome: "retry", scheduledTimeMs, retryCount, errorCode: "DO_RUNTIME_EXCEPTION" };
  }
}

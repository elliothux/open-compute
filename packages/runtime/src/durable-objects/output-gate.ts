import { AsyncLocalStorage } from "node:async_hooks";

/** Cross-module publisher hook used only for committed Durable Object output recovery. */
export const FLUSH_OUTPUT = Symbol.for("open-compute.flush-output");
/** Finalize an already acknowledged output after its local published marker is durable. */
export const FINALIZE_OUTPUT = Symbol.for("open-compute.finalize-output");

export interface OutputPublisher {
  [FLUSH_OUTPUT](payload: Uint8Array, operationId: string): Promise<unknown>;
  [FINALIZE_OUTPUT]?(operationId: string): Promise<void>;
}

const TABLE = "__open_compute_do_output";
const OPERATION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const INTENT_TOKEN = /^[A-Za-z_$][A-Za-z0-9_$-]{0,127}$/;
const ERROR_CODE = /^[A-Z][A-Z0-9_]{0,127}$/;
const als = new AsyncLocalStorage<DoOutputGate>();
type TransactionOutcome = "committed" | "explicit-rollback" | "failed";
interface PendingOutput {
  kind: string;
  publisherName: string;
  payload: Uint8Array;
  operationId: string;
  run: () => Promise<unknown>;
  finalize?: (() => Promise<void>) | undefined;
  resolve?: ((value: unknown) => void) | undefined;
  reject?: ((error: unknown) => void) | undefined;
}

function gateFailure(code: string): Error & { stableCode: string } {
  const error = Object.assign(new Error(code), { stableCode: code });
  error.stack = `${error.name}: ${code}`;
  return error;
}

function publisher(value: unknown): value is OutputPublisher {
  return value !== null && (typeof value === "object" || typeof value === "function")
    && FLUSH_OUTPUT in value && typeof Reflect.get(value, FLUSH_OUTPUT) === "function";
}

function stableCode(error: unknown, fallback: string): string {
  const code = error !== null && typeof error === "object" && "stableCode" in error
    && typeof error.stableCode === "string" ? error.stableCode : fallback;
  return ERROR_CODE.test(code) ? code : fallback;
}

function payloadBytes(raw: unknown): Uint8Array | undefined {
  if (typeof raw === "object" && raw !== null && raw instanceof ArrayBuffer) return new Uint8Array(raw);
  if (ArrayBuffer.isView(raw)) return new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength);
  return undefined;
}

/** Current Durable Object output gate, if the caller is inside a prepared object. */
export function currentOutputGate(): DoOutputGate | undefined {
  return als.getStore();
}

/** Run work with the object-local output gate visible to Queue/Workflow facades. */
export function runWithOutputGate<T>(gate: DoOutputGate, fn: () => T): T {
  return als.run(gate, fn);
}

/**
 * Holds Queue/Workflow mutations until the current storage transaction settles.
 * A thrown/failed transaction drops unpublished work. Cloudflare's explicit
 * `transaction.rollback()` still publishes output when its callback returns, so
 * those intents are atomically restaged after the tenant transaction rolls back.
 */
export class DoOutputGate {
  #storage: DurableObjectStorage;
  #transaction: "async" | "sync" | null = null;
  #syncDepth = 0;
  #pending = new Map<number, PendingOutput>();

  constructor(storage: DurableObjectStorage) {
    this.#storage = storage;
    this.ensureTable();
  }

  /** Recreate the intent table after deleteAll() or first use. */
  ensureTable(): void {
    this.#storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS ${TABLE} (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        publisher TEXT NOT NULL,
        payload BLOB NOT NULL,
        operation_id TEXT NOT NULL UNIQUE,
        state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending', 'published')),
        created_at_ms INTEGER NOT NULL,
        attempt_count INTEGER NOT NULL DEFAULT 0,
        last_error TEXT
      ) STRICT
    `);
  }

  enterTransaction(sync = false): void {
    if (sync && this.#transaction === "sync") {
      this.#syncDepth += 1;
      return;
    }
    if (this.#transaction !== null) throw gateFailure("DO_OUTPUT_GATE_NESTED_TRANSACTION");
    this.#transaction = sync ? "sync" : "async";
    this.#syncDepth = sync ? 1 : 0;
  }

  /** Replace a rolled-back native retry attempt without retaining its closures. */
  retryTransaction(): void {
    if (this.#transaction !== "async") throw gateFailure("DO_OUTPUT_GATE_TRANSACTION_INVALID");
    this.#pending.clear();
  }

  /** Finish a synchronous transaction and publish only rows that survived its commit. */
  exitTransactionSync(): void {
    if (this.#transaction !== "sync") throw gateFailure("DO_OUTPUT_GATE_TRANSACTION_INVALID");
    this.#syncDepth -= 1;
    const retained = new Set(
      this.#storage.sql.exec(`SELECT id FROM ${TABLE}`).toArray().map(row => Number(row.id)),
    );
    for (const [id, pending] of this.#pending) {
      if (retained.has(id)) continue;
      const reject = pending.reject;
      if (reject !== undefined) {
        const error = gateFailure("DO_OUTPUT_GATE_TRANSACTION_ROLLED_BACK");
        // Let transactionSync() return/throw before the mutation Promise
        // settles so its caller can retain and await that exact Promise.
        queueMicrotask(() => reject(error));
      }
      this.#pending.delete(id);
    }
    if (this.#syncDepth > 0) return;
    this.#transaction = null;
    this.#syncDepth = 0;
    // transactionSync() cannot await publication. The exact Promise returned
    // by Queue/Workflow settles after the committed intent is published.
    void this.flush().catch(() => {});
  }

  async exitTransaction(outcome: TransactionOutcome): Promise<void> {
    if (this.#transaction !== "async") throw gateFailure("DO_OUTPUT_GATE_TRANSACTION_INVALID");
    this.#transaction = null;
    if (outcome === "failed") {
      this.#pending.clear();
      return;
    }
    if (outcome === "explicit-rollback") await this.#restageRolledBack();
    await this.flush();
  }

  async #restageRolledBack(): Promise<void> {
    const pending = [...this.#pending.values()];
    const restaged = new Map<number, PendingOutput>();
    this.#storage.transactionSync(() => {
      for (const item of pending) {
        restaged.set(this.#insertIntent(
          item.kind,
          item.publisherName,
          item.payload,
          item.operationId,
        ), item);
      }
    });
    this.#pending = restaged;
    await this.#storage.sync();
  }

  schedule<T>(
    kind: string,
    publisherName: string,
    payload: Uint8Array,
    run: (operationId: string) => Promise<T>,
    staged?: (() => T | Promise<T>) | undefined,
    finalize?: ((operationId: string) => Promise<void>) | undefined,
  ): Promise<T> {
    if (!INTENT_TOKEN.test(kind) || !INTENT_TOKEN.test(publisherName)
        || !(payload instanceof Uint8Array)) {
      throw gateFailure("DO_OUTPUT_GATE_UNPUBLISHABLE");
    }
    if (this.#transaction === "sync") return this.#scheduleSync(
      kind, publisherName, payload, run, finalize,
    );
    return this.#schedule(kind, publisherName, payload, run, staged, finalize);
  }

  #scheduleSync<T>(
    kind: string,
    publisherName: string,
    payload: Uint8Array,
    run: (operationId: string) => Promise<T>,
    finalize?: ((operationId: string) => Promise<void>) | undefined,
  ): Promise<T> {
    this.ensureTable();
    const operationId = crypto.randomUUID();
    const stablePayload = payload.slice();
    const id = this.#insertIntent(kind, publisherName, stablePayload, operationId);
    type Outcome = { ok: true; value: T } | { ok: false; error: unknown };
    let settle!: (outcome: Outcome) => void;
    const settled = new Promise<Outcome>(resolve => {
      settle = resolve;
    });
    const result = settled.then(outcome => {
      if (outcome.ok) return outcome.value;
      throw outcome.error;
    });
    // Preserve rejection for an eventual await while preventing an ignored
    // mutation Promise from becoming an unhandled process-level rejection.
    result.catch(() => {});
    this.#pending.set(id, {
      kind,
      publisherName,
      payload: stablePayload,
      operationId,
      run: () => run(operationId),
      ...(finalize === undefined ? {} : { finalize: () => finalize(operationId) }),
      resolve: value => settle({ ok: true, value: value as T }),
      reject: error => settle({ ok: false, error }),
    });
    return result;
  }

  async #schedule<T>(
    kind: string,
    publisherName: string,
    payload: Uint8Array,
    run: (operationId: string) => Promise<T>,
    staged?: (() => T | Promise<T>) | undefined,
    finalize?: ((operationId: string) => Promise<void>) | undefined,
  ): Promise<T> {
    this.ensureTable();
    const operationId = crypto.randomUUID();
    const stablePayload = payload.slice();
    const id = this.#insertIntent(kind, publisherName, stablePayload, operationId);
    if (this.#transaction === "async") {
      this.#pending.set(id, {
        kind,
        publisherName,
        payload: stablePayload,
        operationId,
        run: () => run(operationId),
        ...(finalize === undefined ? {} : { finalize: () => finalize(operationId) }),
      });
      return staged === undefined ? undefined as T : staged();
    }
    await this.#storage.sync();
    return this.#publish(
      id,
      () => run(operationId),
      finalize === undefined ? undefined : () => finalize(operationId),
    );
  }

  #insertIntent(kind: string, publisherName: string, payload: Uint8Array, operationId: string): number {
    const inserted = this.#storage.sql.exec(
      `INSERT INTO ${TABLE}
         (kind, publisher, payload, operation_id, state, created_at_ms, attempt_count, last_error)
       VALUES (?, ?, ?, ?, 'pending', ?, 0, NULL) RETURNING id`,
      kind,
      publisherName,
      payload,
      operationId,
      Date.now(),
    ).one();
    const id = Number(inserted.id);
    if (!Number.isSafeInteger(id) || id < 1) throw gateFailure("DO_OUTPUT_GATE_UNPUBLISHABLE");
    return id;
  }

  async flush(): Promise<void> {
    this.ensureTable();
    const rows = this.#storage.sql.exec(
      `SELECT id, state FROM ${TABLE} ORDER BY id`,
    ).toArray();
    for (const row of rows) {
      const id = Number(row.id);
      const pending = this.#pending.get(id);
      if (!pending) {
        this.#markFailure(id, "DO_OUTPUT_GATE_RECOVERY_REQUIRED");
        await this.#storage.sync();
        throw gateFailure("DO_OUTPUT_GATE_RECOVERY_REQUIRED");
      }
      try {
        if (row.state === "pending") {
          const value = await this.#publish(id, pending.run, pending.finalize);
          pending.resolve?.(value);
        } else if (row.state === "published" && pending.finalize !== undefined) {
          await this.#finalize(id, pending.finalize);
          pending.resolve?.(undefined);
        } else {
          this.#markFailure(id, "DO_OUTPUT_GATE_UNPUBLISHABLE");
          await this.#storage.sync();
          throw gateFailure("DO_OUTPUT_GATE_UNPUBLISHABLE");
        }
      } catch (error) {
        pending.reject?.(error);
        throw error;
      }
      this.#pending.delete(id);
    }
  }

  async recover(env: Record<string, unknown>): Promise<void> {
    this.ensureTable();
    const rows = this.#storage.sql.exec(
      `SELECT id, kind, publisher, payload, operation_id, state FROM ${TABLE} ORDER BY id`,
    ).toArray();
    for (const row of rows) {
      const id = Number(row.id);
      const operationId = String(row.operation_id);
      const kind = String(row.kind);
      const publisherName = String(row.publisher);
      const target = env[publisherName];
      const payload = payloadBytes(row.payload);
      const state = String(row.state);
      if (!INTENT_TOKEN.test(kind) || !INTENT_TOKEN.test(publisherName)
          || !OPERATION_ID.test(operationId) || !publisher(target) || payload === undefined
          || (state !== "pending" && state !== "published")
          || (state === "published" && typeof target[FINALIZE_OUTPUT] !== "function")) {
        this.#markFailure(id, "DO_OUTPUT_GATE_UNPUBLISHABLE");
        await this.#storage.sync();
        throw gateFailure("DO_OUTPUT_GATE_UNPUBLISHABLE");
      }
      const finalize = typeof target[FINALIZE_OUTPUT] === "function"
        ? () => Reflect.apply(target[FINALIZE_OUTPUT]!, target, [operationId])
        : undefined;
      if (state === "pending") {
        await this.#publish(
          id,
          () => Reflect.apply(target[FLUSH_OUTPUT], target, [payload, operationId]),
          finalize,
        );
      } else {
        await this.#finalize(id, finalize!);
      }
    }
  }

  #markFailure(id: number, code: string): void {
    this.#storage.sql.exec(
      `UPDATE ${TABLE} SET attempt_count = attempt_count + 1, last_error = ? WHERE id = ?`,
      code,
      id,
    );
  }

  async #publish<T>(
    id: number,
    run: () => Promise<T>,
    finalize?: (() => Promise<void>) | undefined,
  ): Promise<T> {
    try {
      const value = await run();
      if (finalize === undefined) {
        this.#storage.sql.exec(`DELETE FROM ${TABLE} WHERE id = ?`, id);
        await this.#storage.sync();
      } else {
        this.#storage.sql.exec(
          `UPDATE ${TABLE} SET state = 'published', last_error = NULL WHERE id = ?`,
          id,
        );
        await this.#storage.sync();
        await this.#finalize(id, finalize);
      }
      return value;
    } catch (error) {
      const state = this.#storage.sql.exec(
        `SELECT state FROM ${TABLE} WHERE id = ?`, id,
      ).toArray()[0]?.state;
      if (state === "published") throw error;
      const code = stableCode(error, "DO_OUTPUT_GATE_PUBLISH_FAILED");
      this.#markFailure(id, code);
      await this.#storage.sync();
      throw gateFailure(code);
    }
  }

  async #finalize(id: number, finalize: () => Promise<void>): Promise<void> {
    try {
      await finalize();
      this.#storage.sql.exec(`DELETE FROM ${TABLE} WHERE id = ?`, id);
      await this.#storage.sync();
    } catch (error) {
      const code = stableCode(error, "DO_OUTPUT_GATE_FINALIZE_FAILED");
      this.#markFailure(id, code);
      await this.#storage.sync();
      throw gateFailure(code);
    }
  }
}

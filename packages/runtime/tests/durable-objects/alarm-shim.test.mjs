import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const asyncHooks = moduleUrl(`
  export class AsyncLocalStorage {
    constructor() { this.stack = []; }
    run(store, fn) {
      this.stack.push(store);
      try {
        const result = fn();
        if (result && typeof result.then === "function") {
          return Promise.resolve(result).finally(() => { this.stack.pop(); });
        }
        this.stack.pop();
        return result;
      } catch (error) {
        this.stack.pop();
        throw error;
      }
    }
    getStore() { return this.stack.at(-1); }
  }
`);
const outputGateUrl = moduleUrl(
  await compileRuntime("durable-objects/output-gate.ts", {
    "node:async_hooks": asyncHooks,
  }),
);
const {
  prepareDurableObjectContext,
  activateDurableObjectAlarm,
  dispatchDurableObjectAlarm,
  repairDurableObjectAlarm,
  runWithOutputGate,
  FLUSH_OUTPUT,
  registerOutputPublisher,
} = await importRuntime("durable-objects/alarm-shim.ts", {
  "./output-gate.js": outputGateUrl,
}).then(async (shim) => ({
  ...shim,
  ...(await import(outputGateUrl)),
}));

function memoryStorage() {
  const kv = new Map();
  const sql = new Map();
  const snapshot = () => ({
    kv: new Map(kv),
    sql: new Map(
      [...sql.entries()].map(([name, rows]) => [
        name,
        rows.map((row) => ({ ...row })),
      ]),
    ),
  });
  const restore = (saved) => {
    kv.clear();
    for (const [key, value] of saved.kv) kv.set(key, value);
    sql.clear();
    for (const [name, rows] of saved.sql)
      sql.set(
        name,
        rows.map((row) => ({ ...row })),
      );
  };
  const exec = (query, ...params) => {
    const text = String(query);
    if (/CREATE TABLE IF NOT EXISTS __open_compute_do_alarm/i.test(text)) {
      if (!sql.has("alarm")) sql.set("alarm", []);
      return cursor([]);
    }
    if (/CREATE TABLE IF NOT EXISTS __open_compute_do_output/i.test(text)) {
      if (!sql.has("output")) sql.set("output", []);
      return cursor([]);
    }
    if (/INSERT INTO __open_compute_do_output/i.test(text)) {
      const rows = sql.get("output") ?? [];
      const id = (rows.at(-1)?.id ?? 0) + 1;
      rows.push({
        id,
        kind: params[0],
        publisher: params[1],
        payload: params[2],
        operation_id: params[3],
        state: "pending",
        created_at_ms: params[4],
        attempt_count: 0,
        last_error: null,
      });
      sql.set("output", rows);
      return cursor([{ id }]);
    }
    if (
      /SELECT (?:id|id, state|id, kind, publisher, payload, operation_id, state) FROM __open_compute_do_output/i.test(
        text,
      )
    ) {
      return cursor([...(sql.get("output") ?? [])]);
    }
    if (/SELECT state FROM __open_compute_do_output WHERE id/i.test(text)) {
      return cursor(
        (sql.get("output") ?? []).filter((row) => row.id === Number(params[0])),
      );
    }
    if (/UPDATE __open_compute_do_output/i.test(text)) {
      const published = /SET state = 'published'/i.test(text);
      const row = (sql.get("output") ?? []).find(
        (value) => value.id === Number(params[published ? 0 : 1]),
      );
      if (row) {
        if (published) {
          row.state = "published";
          row.last_error = null;
        } else {
          row.attempt_count += 1;
          row.last_error = params[0];
        }
      }
      return cursor([]);
    }
    if (/DELETE FROM __open_compute_do_output/i.test(text)) {
      sql.set(
        "output",
        (sql.get("output") ?? []).filter((row) => row.id !== Number(params[0])),
      );
      return cursor([]);
    }
    if (/SELECT id, scheduled_time_ms/i.test(text))
      return cursor([...(sql.get("alarm") ?? [])]);
    if (/INSERT INTO __open_compute_do_alarm/i.test(text)) {
      sql.set("alarm", [
        {
          id: 1,
          scheduled_time_ms: params[0],
          retry_count: 0,
          in_flight: 0,
          row_token: params[1],
          last_error_code: null,
          updated_at_ms: params[2],
        },
      ]);
      return cursor([]);
    }
    if (/DELETE FROM __open_compute_do_alarm/i.test(text)) {
      sql.set("alarm", []);
      return cursor([]);
    }
    if (/DROP /i.test(text)) {
      if (/__open_compute_do_output/i.test(text)) sql.delete("output");
      if (/__open_compute_do_alarm/i.test(text)) sql.delete("alarm");
      return cursor([]);
    }
    if (/sqlite_master/i.test(text)) {
      const names = [...sql.keys()].map((name) => ({
        type: "table",
        name:
          name === "alarm"
            ? "__open_compute_do_alarm"
            : "__open_compute_do_output",
      }));
      return cursor(names);
    }
    if (/PRAGMA/i.test(text)) return cursor([]);
    return cursor([]);
  };
  const storage = {
    kv: {
      get(key) {
        return kv.get(key);
      },
      put(key, value) {
        kv.set(key, value);
      },
      delete(key) {
        return kv.delete(key);
      },
      list() {
        return kv;
      },
    },
    sql: { exec },
    async get(key) {
      return kv.get(key);
    },
    async put(key, value) {
      if (typeof key === "string") kv.set(key, value);
      else Object.entries(key).forEach(([name, item]) => kv.set(name, item));
    },
    async delete(key) {
      if (Array.isArray(key)) {
        let count = 0;
        for (const item of key) if (kv.delete(item)) count += 1;
        return count;
      }
      return kv.delete(key);
    },
    async list() {
      return kv;
    },
    async sync() {},
    async transaction(callback) {
      let saved = snapshot();
      try {
        storage._rollbackRequested = false;
        if (storage._retryOnce === true) {
          storage._retryOnce = false;
          await callback(storage);
          restore(saved);
          await storage._betweenAttempts?.();
          saved = snapshot();
          storage._rollbackRequested = false;
        }
        const value = await callback(storage);
        if (storage._rollbackRequested) restore(saved);
        return value;
      } catch (error) {
        restore(saved);
        throw error;
      }
    },
    transactionSync(callback) {
      const saved = snapshot();
      try {
        return callback(storage);
      } catch (error) {
        restore(saved);
        throw error;
      }
    },
    rollback() {
      storage._rollbackRequested = true;
    },
    async deleteAll() {
      kv.clear();
      sql.clear();
    },
    _sql: sql,
    _kv: kv,
    _retryOnce: false,
    _rollbackRequested: false,
  };
  return storage;
}

function cursor(rows) {
  return {
    toArray() {
      return rows;
    },
    one() {
      return rows[0] ?? {};
    },
    [Symbol.iterator]() {
      return rows[Symbol.iterator]();
    },
  };
}

function context(storage) {
  return {
    storage,
    blockConcurrencyWhile: async (fn) => fn(),
    waitUntil() {},
  };
}

const index = {
  async begin() {},
  async upsert() {},
  async delete() {},
  async clear() {},
};

async function recover(gate, env) {
  for (const binding of Object.values(env))
    registerOutputPublisher(binding, binding);
  return gate.recover(env);
}

test("storage and transaction proxies preserve their native constructor identity", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  assert.equal(prepared.storage.constructor, storage.constructor);
  await prepared.storage.transaction(async (txn) => {
    assert.equal(txn.constructor, storage.constructor);
    await txn.put("receiver", "preserved");
  });
  assert.equal(await prepared.storage.get("receiver"), "preserved");
});

test("thrown transaction failure drops gated mutations and commit publishes once", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  const published = [];
  await runWithOutputGate(prepared.gate, async () => {
    await assert.rejects(
      prepared.storage.transaction(async (txn) => {
        await txn.put("local", "must-rollback");
        const staged = await prepared.gate.schedule(
          "queue",
          "EVENTS",
          new Uint8Array([1]),
          async () => {
            published.push("one");
            return "one";
          },
          () => "staged",
        );
        assert.equal(staged, "staged");
        throw new Error("rollback");
      }),
      /rollback/,
    );
    assert.equal(await prepared.storage.get("local"), undefined);
    assert.deepEqual(published, []);
    await prepared.storage.transaction(async (txn) => {
      await txn.put("local", "committed");
      const staged = await prepared.gate.schedule(
        "queue",
        "EVENTS",
        new Uint8Array([2]),
        async () => {
          published.push("two");
          return "two";
        },
        () => "staged",
      );
      assert.equal(staged, "staged");
      assert.deepEqual(published, []);
    });
  });
  assert.equal(await prepared.storage.get("local"), "committed");
  assert.deepEqual(published, ["two"]);
});

test("publish failure after commit leaves intent for recover exactly once", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  await runWithOutputGate(prepared.gate, async () => {
    await assert.rejects(
      prepared.storage.transaction(async () => {
        await prepared.gate.schedule(
          "queue",
          "EVENTS",
          new Uint8Array([9]),
          async () => {
            throw new Error("crash-before-ack");
          },
          () => undefined,
        );
      }),
      /DO_OUTPUT_GATE_PUBLISH_FAILED/,
    );
  });
  const flushed = [];
  await recover(prepared.gate, {
    EVENTS: {
      async [FLUSH_OUTPUT](payload) {
        flushed.push(Array.from(payload));
      },
    },
  });
  assert.deepEqual(flushed, [[9]]);
  assert.equal((storage._sql.get("output") ?? []).length, 0);
});

test("native transaction retry discards the rolled-back attempt closure", async () => {
  const storage = memoryStorage();
  storage._retryOnce = true;
  const prepared = prepareDurableObjectContext(context(storage), index);
  const published = [];
  let attempts = 0;
  await runWithOutputGate(prepared.gate, async () => {
    await prepared.storage.transaction(async () => {
      attempts += 1;
      const attempt = attempts;
      await prepared.gate.schedule(
        "queue",
        "EVENTS",
        new Uint8Array([attempt]),
        async () => {
          published.push(attempt);
        },
        () => undefined,
      );
    });
  });
  assert.equal(attempts, 2);
  assert.deepEqual(published, [2]);
});

test("explicit transaction rollback publishes staged output while reverting storage", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  const published = [];
  await runWithOutputGate(prepared.gate, async () => {
    const value = await prepared.storage.transaction(async (txn) => {
      const staged = await prepared.gate.schedule(
        "queue",
        "EVENTS",
        new Uint8Array([8]),
        async () => {
          published.push("published");
        },
        () => "staged",
      );
      txn.rollback();
      return staged;
    });
    assert.equal(value, "staged");
  });
  assert.deepEqual(published, ["published"]);
  assert.equal((storage._sql.get("output") ?? []).length, 0);
});

test("transactionSync publishes committed output and rejects rolled-back output", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  const published = [];
  let committed;
  assert.equal(
    prepared.storage.transactionSync(() => {
      committed = prepared.gate.schedule(
        "queue",
        "EVENTS",
        new Uint8Array([5]),
        async () => {
          published.push("committed");
          return "published";
        },
      );
      return "sync-result";
    }),
    "sync-result",
  );
  assert.equal(await committed, "published");
  assert.deepEqual(published, ["committed"]);
  let rolledBack;
  assert.throws(
    () =>
      prepared.storage.transactionSync(() => {
        rolledBack = prepared.gate.schedule(
          "queue",
          "EVENTS",
          new Uint8Array([6]),
          async () => {
            published.push("must-not-publish");
          },
        );
        throw new Error("rollback");
      }),
    /rollback/,
  );
  await assert.rejects(rolledBack, /DO_OUTPUT_GATE_TRANSACTION_ROLLED_BACK/);
  assert.deepEqual(published, ["committed"]);
  assert.equal((storage._sql.get("output") ?? []).length, 0);
});

test("transactionSync uses the stable zero-argument callback and rejects alarms synchronously", () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  let callbackArgument = "not-called";
  assert.throws(
    () =>
      prepared.storage.transactionSync((argument) => {
        callbackArgument = argument;
        prepared.storage.setAlarm(Date.now() + 60_000);
      }),
    /setAlarm\(\) is not supported inside transactionSync\(\)/,
  );
  assert.equal(callbackArgument, undefined);
  assert.equal((storage._sql.get("alarm") ?? []).length, 0);
});

test("committed transaction keeps its alarm authority when index projection is unavailable", async () => {
  const storage = memoryStorage();
  const failingIndex = {
    ...index,
    async upsert() {
      throw new Error("index-down");
    },
    async delete() {
      throw new Error("index-down");
    },
    async clear() {
      throw new Error("index-down");
    },
  };
  const prepared = prepareDurableObjectContext(context(storage), failingIndex);
  const scheduled = Date.now() + 60_000;
  await prepared.storage.transaction(async (txn) => {
    await txn.put("committed", true);
    await txn.setAlarm(scheduled);
  });
  assert.equal(await prepared.storage.get("committed"), true);
  assert.equal(await prepared.storage.getAlarm(), scheduled);
  assert.equal(
    (await repairDurableObjectAlarm(prepared)).scheduledTimeMs,
    scheduled,
  );
});

test("alarm dispatch applies six exponential retries then exhausts", async () => {
  const storage = memoryStorage();
  const projections = [];
  const marks = [];
  const retryIndex = {
    ...index,
    async begin(revision) {
      marks.push(revision);
    },
    async upsert(value) {
      projections.push(value);
    },
    async delete() {},
    async clear() {},
  };
  const prepared = prepareDurableObjectContext(context(storage), retryIndex);
  const originalNow = Date.now;
  let now = 10_000;
  Date.now = () => now;
  try {
    await prepared.storage.setAlarm(now - 1);
    const row = storage._sql.get("alarm")[0];
    for (let retryCount = 0; retryCount < 6; retryCount += 1) {
      const result = await dispatchDurableObjectAlarm(
        {},
        async function alarm() {
          throw new Error("retry");
        },
        prepared,
        { rowToken: row.row_token, retryCount },
      );
      assert.equal(result.outcome, "retry");
      assert.equal(result.retryCount, retryCount + 1);
      assert.equal(result.scheduledTimeMs, now + 2_000 * 2 ** retryCount);
      now = result.scheduledTimeMs;
    }
    const exhausted = await dispatchDurableObjectAlarm(
      {},
      async function alarm() {
        throw new Error("exhausted");
      },
      prepared,
      { rowToken: row.row_token, retryCount: 6 },
    );
    assert.equal(exhausted.outcome, "exhausted");
    assert.equal((storage._sql.get("alarm") ?? []).length, 0);
    assert.equal(projections.length, 7);
    assert.equal(new Set(marks).size, 8);
  } finally {
    Date.now = originalNow;
  }
});

test("deleteAll refuses index failure before deleting tenant state", async () => {
  const storage = memoryStorage();
  const scheduled = Date.now() + 60_000;
  const prepared = prepareDurableObjectContext(context(storage), index);
  await prepared.storage.put("user", true);
  await prepared.storage.setAlarm(scheduled);
  const rejectingIndex = {
    ...index,
    async upsert() {},
    async delete() {
      throw new Error("index-down");
    },
    async clear() {
      throw new Error("index-down");
    },
  };
  const rejecting = prepareDurableObjectContext(
    context(storage),
    rejectingIndex,
  );
  await assert.rejects(
    rejecting.storage.deleteAll(),
    /DO_ALARM_INDEX_UNAVAILABLE/,
  );
  assert.equal(await rejecting.storage.get("user"), true);
  assert.equal(await rejecting.storage.getAlarm(), scheduled);
});

test("deleteAll drops alarms then recreates internal gate tables", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  await prepared.storage.setAlarm(Date.now() + 60_000);
  await prepared.storage.put("user", true);
  await prepared.storage.deleteAll();
  assert.equal(await prepared.storage.get("user"), undefined);
  assert.equal(await prepared.storage.getAlarm(), null);
  const published = [];
  await runWithOutputGate(prepared.gate, async () => {
    await prepared.gate.schedule(
      "queue",
      "EVENTS",
      new Uint8Array([3]),
      async () => {
        published.push("after-delete-all");
        return "ok";
      },
    );
  });
  assert.deepEqual(published, ["after-delete-all"]);
});

test("deleteAll marks each retry with an alarm and updates only the relevant projection", async () => {
  const first = crypto.randomUUID();
  const second = crypto.randomUUID();
  for (const [initial, retry, expected] of [
    [null, first, ["clear", "begin", first]],
    [first, first, ["begin", first, "begin"]],
    [first, second, ["begin", first, "begin", second]],
    [first, null, ["begin", first, "clear"]],
  ]) {
    const raw = memoryStorage();
    const events = [];
    const prepared = prepareDurableObjectContext(context(raw), {
      ...index,
      async begin() {
        assert.equal(raw._kv.get("ordinary"), true);
        assert.equal(raw._sql.get("alarm").length, 1);
        events.push("begin");
      },
      async delete(token) {
        assert.equal(raw._sql.get("alarm")[0].row_token, token);
        events.push(token);
      },
      async clear() {
        assert.equal(raw._sql.get("alarm").length, 0);
        events.push("clear");
      },
    });
    const seedAlarm = (token) => {
      raw._sql.set(
        "alarm",
        token === null
          ? []
          : [
              {
                id: 1,
                scheduled_time_ms: 1,
                retry_count: 0,
                in_flight: 0,
                row_token: token,
                last_error_code: null,
                updated_at_ms: 1,
              },
            ],
      );
    };
    await prepared.storage.put("ordinary", true);
    seedAlarm(initial);
    raw._retryOnce = true;
    raw._betweenAttempts = () => seedAlarm(retry);

    await prepared.storage.deleteAll();

    assert.deepEqual(events, expected);
    assert.equal(await prepared.storage.get("ordinary"), undefined);
    assert.deepEqual(await repairDurableObjectAlarm(prepared), {
      exists: false,
    });
  }
});

test("deleteAll rollback restores the projection for the row that survived a retry", async () => {
  const raw = memoryStorage();
  const restored = [];
  const prepared = prepareDurableObjectContext(context(raw), {
    ...index,
    async upsert(row) {
      restored.push(row.rowToken);
    },
  });
  await prepared.storage.setAlarm(1);
  await prepared.storage.put("ordinary", true);
  restored.length = 0;
  const retryToken = crypto.randomUUID();
  raw._retryOnce = true;
  raw._betweenAttempts = () => {
    raw._sql.get("alarm")[0].row_token = retryToken;
    const exec = raw.sql.exec;
    raw.sql.exec = (query, ...args) => {
      if (query.startsWith("DROP ")) throw new Error("local-delete-failed");
      return exec(query, ...args);
    };
  };

  await assert.rejects(prepared.storage.deleteAll(), /local-delete-failed/);

  assert.deepEqual(restored, [retryToken]);
  assert.equal(await prepared.storage.get("ordinary"), true);
  assert.equal((await repairDurableObjectAlarm(prepared)).rowToken, retryToken);
});

test("corrupt alarm authority fails closed without deleting the row", async () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  storage._sql.set("alarm", [
    {
      id: 1,
      scheduled_time_ms: "bad",
      retry_count: 0,
      in_flight: 0,
      row_token: "nope",
      last_error_code: null,
      updated_at_ms: 1,
    },
  ]);
  await assert.rejects(prepared.storage.getAlarm(), /DO_STORAGE_UNAVAILABLE/);
  assert.equal(storage._sql.get("alarm").length, 1);
});

test("tenant SQL cannot read or mutate private alarm and output authority", () => {
  const storage = memoryStorage();
  const prepared = prepareDurableObjectContext(context(storage), index);
  assert.throws(
    () => prepared.storage.sql.exec("SELECT * FROM __open_compute_do_alarm"),
    /SQLITE_AUTH/,
  );
  assert.throws(
    () => prepared.storage.sql.exec("DROP TABLE __OPEN_COMPUTE_DO_OUTPUT"),
    /SQLITE_AUTH/,
  );
});

test("tenant global edits cannot read native storage or the private alarm index", async () => {
  const raw = memoryStorage();
  const ctx = { storage: raw, props: {}, exports: {} };
  const index = {
    async begin() {},
    async upsert() {},
    async delete() {},
    async clear() {},
  };
  const secrets = new Set([raw, ctx, index]);
  const observed = [];
  const get = Reflect.get;
  const apply = Reflect.apply;
  const freeze = Object.freeze;
  const ProxyConstructor = globalThis.Proxy;
  try {
    Reflect.get = (target, ...args) => {
      if (secrets.has(target)) observed.push("get");
      return get(target, ...args);
    };
    Reflect.apply = (method, receiver, args) => {
      if (secrets.has(receiver)) observed.push("apply");
      return apply(method, receiver, args);
    };
    Object.freeze = (value) => {
      if (value?.index === index || value?.rawStorage === raw)
        observed.push("freeze");
      return freeze(value);
    };
    globalThis.Proxy = function (target, handler) {
      if (secrets.has(target)) observed.push("Proxy");
      return new ProxyConstructor(target, handler);
    };
    const prepared = prepareDurableObjectContext(ctx, index);
    assert.equal(await prepared.storage.get("absent"), undefined);
    assert.equal(prepared.index, index);
  } finally {
    Reflect.get = get;
    Reflect.apply = apply;
    Object.freeze = freeze;
    globalThis.Proxy = ProxyConstructor;
  }
  assert.deepEqual(observed, []);
});

test("private alarm activation retains the native input-gate receiver before tenant replacement", async () => {
  const ctx = context(memoryStorage());
  const pending = [];
  let cleared = 0;
  ctx.blockConcurrencyWhile = function (callback) {
    assert.equal(this, ctx);
    const task = Promise.resolve().then(callback);
    pending.push(task);
    return task;
  };
  const prepared = prepareDurableObjectContext(ctx, {
    ...index,
    async clear() {
      cleared += 1;
    },
  });
  ctx.blockConcurrencyWhile = () => {
    throw new Error("tenant replacement reached private activation");
  };
  activateDurableObjectAlarm(prepared, {});
  await Promise.all(pending);
  assert.equal(cleared, 1);
  assert.equal(pending.length, 1);
});

test("empty object access and stale delivery do not mark alarm authority", async () => {
  let marks = 0;
  let clears = 0;
  const prepared = prepareDurableObjectContext(context(memoryStorage()), {
    ...index,
    async begin() {
      marks += 1;
    },
    async clear() {
      clears += 1;
    },
  });
  activateDurableObjectAlarm(prepared);
  assert.equal(await prepared.storage.getAlarm(), null);
  await prepared.storage.deleteAlarm();
  await prepared.storage.transaction(async (txn) => {
    await txn.put("ordinary", true);
    await txn.deleteAlarm();
  });
  assert.deepEqual(
    await dispatchDurableObjectAlarm({}, undefined, prepared, {
      rowToken: crypto.randomUUID(),
      retryCount: 0,
    }),
    { outcome: "stale" },
  );
  assert.deepEqual(await repairDurableObjectAlarm(prepared), { exists: false });
  assert.equal(marks, 0);

  const clearsBeforeDeleteAll = clears;
  await prepared.storage.deleteAll();
  assert.equal(marks, 0);
  assert.equal(clears, clearsBeforeDeleteAll + 1);
  assert.equal(await prepared.storage.get("ordinary"), undefined);
});

test("failed begin leaves local state untouched for every alarm mutation", async () => {
  const raw = memoryStorage();
  let fail = true;
  let projected = 0;
  const prepared = prepareDurableObjectContext(context(raw), {
    ...index,
    async begin() {
      if (fail) throw new Error("index-down");
    },
    async upsert() {
      projected += 1;
    },
    async delete() {
      projected += 1;
    },
  });
  const storage = prepared.storage;
  await assert.rejects(storage.setAlarm(1), /DO_ALARM_INDEX_UNAVAILABLE/);
  assert.deepEqual(await repairDurableObjectAlarm(prepared), { exists: false });

  fail = false;
  await storage.setAlarm(1);
  await storage.put("value", 1);
  const before = await repairDurableObjectAlarm(prepared);
  fail = true;
  await assert.rejects(storage.deleteAlarm(), /DO_ALARM_INDEX_UNAVAILABLE/);
  await assert.rejects(storage.deleteAll(), /DO_ALARM_INDEX_UNAVAILABLE/);
  await assert.rejects(
    storage.transaction(async (txn) => {
      await txn.put("value", 2);
      await txn.setAlarm(2);
    }),
    /DO_ALARM_INDEX_UNAVAILABLE/,
  );
  let delivered = false;
  await assert.rejects(
    dispatchDurableObjectAlarm(
      {},
      () => {
        delivered = true;
      },
      prepared,
      {
        rowToken: before.rowToken,
        retryCount: 0,
      },
    ),
    /DO_ALARM_INDEX_UNAVAILABLE/,
  );
  assert.equal(delivered, false);
  assert.equal(raw._sql.get("alarm")[0].in_flight, 0);
  assert.equal(await storage.get("value"), 1);
  assert.deepEqual(await repairDurableObjectAlarm(prepared), before);
  assert.equal(projected, 1);
});

test("alarm transaction rollback keeps the original row and skips projection", async () => {
  const events = [];
  const prepared = prepareDurableObjectContext(context(memoryStorage()), {
    ...index,
    async begin() {
      events.push("begin");
    },
    async upsert() {
      events.push("upsert");
    },
    async delete() {
      events.push("delete");
    },
  });
  await prepared.storage.setAlarm(1);
  const before = await repairDurableObjectAlarm(prepared);
  await prepared.storage.transaction(async (txn) => {
    await txn.deleteAlarm();
    txn.rollback();
  });
  assert.deepEqual(await repairDurableObjectAlarm(prepared), before);
  assert.deepEqual(events, ["begin", "upsert", "begin"]);
});

test(
  "repair rejects during begin, retry, and post-commit projection",
  { timeout: 2000 },
  async () => {
    const raw = memoryStorage();
    const beginStarted = Promise.withResolvers();
    const finishBegin = Promise.withResolvers();
    const projectionStarted = Promise.withResolvers();
    const finishProjection = Promise.withResolvers();
    const marks = [];
    const prepared = prepareDurableObjectContext(context(raw), {
      ...index,
      async begin(revision) {
        marks.push(revision);
        if (marks.length === 1) {
          beginStarted.resolve();
          await finishBegin.promise;
        }
      },
      async upsert(row) {
        projectionStarted.resolve(row);
        await finishProjection.promise;
      },
    });
    const assertBusy = () =>
      assert.rejects(
        repairDurableObjectAlarm(prepared),
        /DO_ALARM_INDEX_UNAVAILABLE/,
      );
    raw._retryOnce = true;
    raw._betweenAttempts = assertBusy;
    const transaction = prepared.storage.transaction(async (txn) => {
      await Promise.all([txn.setAlarm(1), txn.setAlarm(2)]);
      await assertBusy();
      await txn.deleteAlarm();
      await txn.setAlarm(3);
    });
    try {
      await beginStarted.promise;
      assert.equal(raw._sql.get("alarm").length, 0);
      await assertBusy();
      finishBegin.resolve();

      const projected = await projectionStarted.promise;
      assert.equal(raw._sql.get("alarm")[0].row_token, projected.rowToken);
      assert.equal(projected.scheduledTimeMs, 3);
      await assertBusy();
      assert.equal(marks.length, 2);
      assert.equal(new Set(marks).size, 2);
    } finally {
      finishBegin.resolve();
      finishProjection.resolve();
      await transaction;
    }
    assert.equal((await repairDurableObjectAlarm(prepared)).scheduledTimeMs, 3);
  },
);

test("failed standalone projection removes the unprojected local row", async () => {
  const raw = memoryStorage();
  let prepared;
  prepared = prepareDurableObjectContext(context(raw), {
    ...index,
    async begin() {
      assert.equal(raw._sql.get("alarm").length, 0);
    },
    async upsert(row) {
      assert.equal(raw._sql.get("alarm")[0].row_token, row.rowToken);
      await assert.rejects(
        repairDurableObjectAlarm(prepared),
        /DO_ALARM_INDEX_UNAVAILABLE/,
      );
      throw new Error("index-down");
    },
  });
  await assert.rejects(
    prepared.storage.setAlarm(1),
    /DO_ALARM_INDEX_UNAVAILABLE/,
  );
  assert.deepEqual(await repairDurableObjectAlarm(prepared), { exists: false });
});

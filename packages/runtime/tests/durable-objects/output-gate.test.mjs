import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

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
const {
  DoOutputGate, runWithOutputGate, currentOutputGate, FLUSH_OUTPUT, FINALIZE_OUTPUT,
} = await importRuntime(
  "durable-objects/output-gate.ts",
  { "node:async_hooks": asyncHooks },
);

function sqlStorage() {
  const tables = new Map();
  const exec = (query, ...params) => {
    const text = String(query);
    if (text.includes("CREATE TABLE")) return { toArray() { return []; }, one() { return {}; } };
    if (text.includes("INSERT")) {
      const rows = tables.get("output") ?? [];
      const id = rows.length + 1;
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
      tables.set("output", rows);
      return { one() { return { id }; }, toArray() { return [{ id }]; } };
    }
    if (text.includes("UPDATE")) {
      const published = /SET state = 'published'/i.test(text);
      const row = (tables.get("output") ?? []).find(value => value.id === Number(params[published ? 0 : 1]));
      if (row) {
        if (published) {
          row.state = "published";
          row.last_error = null;
        } else {
          row.attempt_count += 1;
          row.last_error = params[0];
        }
      }
      return { toArray() { return []; }, one() { return {}; } };
    }
    if (text.includes("DELETE")) {
      const id = Number(params[0]);
      tables.set("output", (tables.get("output") ?? []).filter(row => row.id !== id));
      return { toArray() { return []; }, one() { return {}; } };
    }
    const rows = /WHERE id = \?/i.test(text)
      ? (tables.get("output") ?? []).filter(row => row.id === Number(params[0]))
      : tables.get("output") ?? [];
    return {
      toArray() { return rows.map(row => ({ ...row })); },
      one() { return rows[0] ?? {}; },
    };
  };
  return {
    sql: { exec },
    async sync() {},
    _tables: tables,
  };
}

test("awaited transaction mutation stages immediately and commit publishes once", async () => {
  const storage = sqlStorage();
  const gate = new DoOutputGate(storage);
  const published = [];
  await runWithOutputGate(gate, async () => {
    assert.equal(currentOutputGate(), gate);
    gate.enterTransaction();
    const first = await gate.schedule("queue", "EVENTS", new Uint8Array([1]), async () => {
      published.push("one");
      return "one";
    }, () => "staged");
    assert.equal(first, "staged");
    assert.deepEqual(published, []);
    await gate.exitTransaction("committed");
    assert.deepEqual(published, ["one"]);
    const second = await gate.schedule("queue", "EVENTS", new Uint8Array([2]), async () => {
      published.push("two");
      return "two";
    });
    assert.equal(second, "two");
    assert.deepEqual(published, ["one", "two"]);
  });
});

test("older recovered intent blocks a newer committed closure", async () => {
  const storage = sqlStorage();
  storage._tables.set("output", [{
    id: 1,
    kind: "queue",
    publisher: "EVENTS",
    payload: new Uint8Array([1]),
    operation_id: crypto.randomUUID(),
    state: "pending",
    created_at_ms: 1,
    attempt_count: 0,
    last_error: null,
  }]);
  const gate = new DoOutputGate(storage);
  const published = [];
  gate.enterTransaction();
  await gate.schedule("queue", "EVENTS", new Uint8Array([2]), async () => {
    published.push("newer");
  }, () => undefined);
  await assert.rejects(gate.exitTransaction("committed"), /DO_OUTPUT_GATE_RECOVERY_REQUIRED/);
  assert.deepEqual(published, []);
  assert.equal(storage._tables.get("output")[0].attempt_count, 1);
  assert.equal(storage._tables.get("output")[0].last_error, "DO_OUTPUT_GATE_RECOVERY_REQUIRED");
});

test("recover publishes remaining committed intents through the named publisher", async () => {
  const storage = sqlStorage();
  const gate = new DoOutputGate(storage);
  await gate.schedule("queue", "EVENTS", new Uint8Array([9, 9]), async () => {
    throw new Error("crash-before-ack");
  }).catch(() => undefined);
  const flushed = [];
  const env = {
    EVENTS: {
      async [FLUSH_OUTPUT](payload) { flushed.push(Array.from(payload)); },
    },
  };
  await gate.recover(env);
  assert.deepEqual(flushed, [[9, 9]]);
  assert.equal((storage._tables.get("output") ?? []).length, 0);
});

test("failed publish records retry evidence and recovery keeps the operation identity", async () => {
  const storage = sqlStorage();
  const gate = new DoOutputGate(storage);
  let firstOperation;
  await assert.rejects(gate.schedule("queue", "EVENTS", new Uint8Array([7]), async operationId => {
    firstOperation = operationId;
    throw Object.assign(new Error("unavailable"), { stableCode: "QUEUE_STORAGE_UNAVAILABLE" });
  }), /QUEUE_STORAGE_UNAVAILABLE/);
  const [intent] = storage._tables.get("output");
  assert.equal(intent.operation_id, firstOperation);
  assert.equal(intent.attempt_count, 1);
  assert.equal(intent.last_error, "QUEUE_STORAGE_UNAVAILABLE");
  let recoveredOperation;
  await gate.recover({
    EVENTS: {
      async [FLUSH_OUTPUT](_payload, operationId) { recoveredOperation = operationId; },
    },
  });
  assert.equal(recoveredOperation, firstOperation);
  assert.equal((storage._tables.get("output") ?? []).length, 0);
});

test("malformed committed intent fails closed and remains retryable", async () => {
  const storage = sqlStorage();
  const gate = new DoOutputGate(storage);
  storage._tables.set("output", [{
    id: 1,
    kind: "queue",
    publisher: "EVENTS",
    payload: new Uint8Array([1]),
    operation_id: "not-an-operation",
    state: "pending",
    created_at_ms: 1,
    attempt_count: 0,
    last_error: null,
  }]);
  await assert.rejects(gate.recover({ EVENTS: { async [FLUSH_OUTPUT]() {} } }),
    /DO_OUTPUT_GATE_UNPUBLISHABLE/);
  const [intent] = storage._tables.get("output");
  assert.equal(intent.attempt_count, 1);
  assert.equal(intent.last_error, "DO_OUTPUT_GATE_UNPUBLISHABLE");
});

test("acknowledged output is finalized before durable local deletion", async () => {
  const storage = sqlStorage();
  const gate = new DoOutputGate(storage);
  const events = [];
  await gate.schedule("queue", "EVENTS", new Uint8Array([4]), async () => {
    events.push("publish");
    return "ok";
  }, undefined, async () => {
    assert.equal(storage._tables.get("output")[0].state, "published");
    events.push("finalize");
  });
  assert.deepEqual(events, ["publish", "finalize"]);
  assert.equal((storage._tables.get("output") ?? []).length, 0);
});

test("finalize failure recovery never republishes an acknowledged output", async () => {
  const storage = sqlStorage();
  const gate = new DoOutputGate(storage);
  let publishes = 0;
  let finalizes = 0;
  await assert.rejects(gate.schedule("queue", "EVENTS", new Uint8Array([6]), async () => {
    publishes += 1;
  }, undefined, async () => {
    finalizes += 1;
    throw new Error("finalize-response-lost");
  }), /DO_OUTPUT_GATE_FINALIZE_FAILED/);
  const [published] = storage._tables.get("output");
  assert.equal(published.state, "published");
  assert.equal(published.attempt_count, 1);
  await gate.recover({
    EVENTS: {
      async [FLUSH_OUTPUT]() { publishes += 1; },
      async [FINALIZE_OUTPUT]() { finalizes += 1; },
    },
  });
  assert.equal(publishes, 1);
  assert.equal(finalizes, 2);
  assert.equal((storage._tables.get("output") ?? []).length, 0);
});

import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const json = moduleUrl(await compileRuntime("bindings/json-body.ts"));
const native = moduleUrl(
  await compileRuntime("d1/native-adapter.ts", {
    "../bindings/json-body.js": json,
    "./validation.js": moduleUrl(await compileRuntime("d1/validation.ts")),
  }),
);
const { makeD1TransportBase } = await importRuntime("d1/transport.ts", {
  "cloudflare:workers": moduleUrl(
    "export class WorkerEntrypoint { constructor(ctx,env) { this.ctx=ctx;this.env=env; } }",
  ),
  "./native-adapter.js": native,
});
const Base = makeD1TransportBase(
  (code) => Object.assign(new Error(code), { stableCode: code }),
  () => "generation",
  "token",
);
const meta = {
  duration: 0,
  size_after: 0,
  rows_read: 1,
  rows_written: 0,
  last_row_id: 0,
  changed_db: false,
  changes: 0,
};

test("upstream D1 HTTP/RPC wire preserves results, blobs, sessions and sanitized errors", async () => {
  const calls = [];
  class Transport extends Base {
    async executeStatements(mode, statements, session) {
      calls.push({ mode, statements, session });
      return {
        results: statements.map(() => ({
          columns: ["value", "blob"],
          rows: [[1, new Uint8Array([2, 3])]],
          meta,
        })),
        bookmark: session.kind === 0 ? null : "bookmark",
        stateVersion: 1,
      };
    }
  }
  const transport = new Transport();
  const fetch = (body, format = "ROWS_AND_COLUMNS", bookmark) =>
    transport.fetch(
      new Request(`http://d1/query?resultsFormat=${format}`, {
        method: "POST",
        body: JSON.stringify(body),
        headers:
          bookmark === undefined
            ? {}
            : { "x-cf-d1-session-commit-token": bookmark },
      }),
    );
  const response = await fetch(
    { sql: "SELECT ?", params: [[4, 5], "x", 1, null] },
    "ROWS_AND_COLUMNS",
    "first-primary",
  );
  assert.equal(
    response.headers.get("x-cf-d1-session-commit-token"),
    "bookmark",
  );
  assert.deepEqual(await response.json(), {
    success: true,
    meta,
    results: { columns: ["value", "blob"], rows: [[1, [2, 3]]] },
  });
  assert.deepEqual(calls.at(-1), {
    mode: "all",
    statements: [
      { sql: "SELECT ?", params: [new Uint8Array([4, 5]), "x", 1, null] },
    ],
    session: { kind: 2 },
  });
  assert.deepEqual(
    (await (await fetch({ sql: "SELECT 1" }, "ARRAY_OF_OBJECTS")).json())
      .results,
    [{ value: 1, blob: [2, 3] }],
  );
  assert.deepEqual(
    (await (await fetch({ sql: "SELECT 1" }, "NONE")).json()).results,
    [],
  );
  assert.equal(calls.at(-1).mode, "run");
  assert.equal(
    (await (await fetch([{ sql: "one" }, { sql: "two" }])).json()).length,
    2,
  );
  assert.equal(calls.at(-1).mode, "batch");
  const rpc = await transport.query({
    queries: [{ sql: "SELECT 1" }],
    bookmark: "first-unconstrained",
  });
  assert.deepEqual(rpc, {
    success: true,
    results: {
      queryResults: [
        {
          meta,
          data: {
            kind: "raw",
            columns: ["value", "blob"],
            rows: [[1, [2, 3]]],
          },
        },
      ],
      bookmark: "bookmark",
    },
  });
  assert.deepEqual(calls.at(-1).session, { kind: 1 });
  await transport.query({ queries: [{ sql: "SELECT 1" }], bookmark: "sealed" });
  assert.deepEqual(calls.at(-1).session, { kind: 3, bookmark: "sealed" });
  for (const input of [
    null,
    {},
    [],
    { queries: [{ sql: "x", params: [{}] }] },
    { queries: [{ sql: "x", params: [Infinity] }] },
    { queries: [{ sql: "x" }], bookmark: "" },
  ]) {
    const failed = await transport.query(input);
    assert.equal(failed.success, false);
    assert.match(failed.error.message, /^D1_(?:TYPE|SESSION)_ERROR$/);
  }
  assert.equal(
    (await transport.query({ queries: [{ sql: 3 }, {}] })).error.message,
    'Malformed input: [{"sql":3},{}], should be {sql: string, params?: any[]} or an array of these query objects',
  );
  for (const queries of [[], [{ sql: "" }], [{ sql: "  " }]]) {
    assert.equal(
      (await transport.query({ queries })).error.message,
      "No SQL statements detected.",
    );
  }
  for (const request of [
    new Request("http://d1/query"),
    new Request("http://other/query", { method: "POST" }),
    new Request("http://d1/dump", { method: "POST" }),
    new Request("http://d1/query?resultsFormat=bad", { method: "POST" }),
  ]) {
    assert.deepEqual(await (await transport.fetch(request)).json(), {
      success: false,
      error: "D1_TYPE_ERROR",
    });
  }
  const invalid = await transport.fetch(
    new Request("http://d1/query", { method: "POST", body: "{" }),
  );
  assert.deepEqual(await invalid.json(), {
    success: false,
    error: "D1_LIMIT_ERROR",
  });
  const oversized = await transport.fetch(
    new Request("http://d1/query", {
      method: "POST",
      body: "x".repeat(16 * 1024 * 1024 + 1),
    }),
  );
  assert.deepEqual(await oversized.json(), {
    success: false,
    error: "D1_LIMIT_ERROR",
  });
  const denied = new Base(
    {
      props: {
        bindingId: "binding",
        versionId: "version",
        descriptorSha256: "a".repeat(64),
        resourceSpecGeneration: 1,
        permissions: { read: false, write: false },
      },
    },
    {
      BINDING_BACKEND: {
        fetch() {
          throw new Error("must not reach backend");
        },
      },
    },
  );
  assert.equal(
    (await denied.query({ queries: [{ sql: "SELECT 1" }] })).error.message,
    "BINDING_PERMISSION_DENIED",
  );
  class Failing extends Base {
    async executeStatements() {
      throw new Error("D1_PRIVATE_SECRET");
    }
  }
  assert.equal(
    (await new Failing().query({ queries: [{ sql: "x" }] })).error.message,
    "D1_INTERNAL_PROTOCOL_ERROR",
  );
  class Corrupt extends Base {
    async executeStatements() {
      return { results: [], bookmark: null, stateVersion: 0 };
    }
  }
  assert.equal(
    (await new Corrupt().query({ queries: [{ sql: "x" }] })).error.message,
    "D1_INTERNAL_PROTOCOL_ERROR",
  );
});

test("D1 prepared run preserves bound values, SELECT rows, write metadata and sessions", async () => {
  const calls = [];
  const writeMeta = {
    ...meta,
    rows_read: 0,
    rows_written: 1,
    changed_db: true,
    changes: 1,
  };
  class Transport extends Base {
    async exec() {
      assert.fail("prepared run must use statement authority");
    }
    async executeStatements(mode, statements, session) {
      calls.push({ mode, statements, session });
      const { sql, params } = statements[0];
      if (sql === "broken") throw new Error("private SQL and secret");
      return {
        results: [
          sql === "SELECT ?, ?"
            ? { columns: ["value", "blob"], rows: [params], meta }
            : { columns: [], rows: [], meta: writeMeta },
        ],
        bookmark: session.kind === 0 ? null : "sealed",
        stateVersion: 1,
      };
    }
  }
  const transport = new Transport();
  const run = (input, bookmark) =>
    transport.fetch(
      new Request("http://d1/execute?resultsFormat=NONE", {
        method: "POST",
        body: JSON.stringify(input),
        headers:
          bookmark === undefined
            ? {}
            : { "x-cf-d1-session-commit-token": bookmark },
      }),
    );
  const response = await run(
    { sql: "SELECT ?, ?", params: ["row", [2, 3, 255]] },
    "first-primary",
  );
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("x-cf-d1-session-commit-token"), "sealed");
  assert.deepEqual(await response.json(), {
    success: true,
    meta,
    results: [{ value: "row", blob: [2, 3, 255] }],
  });
  assert.deepEqual(calls.at(-1), {
    mode: "all",
    statements: [
      { sql: "SELECT ?, ?", params: ["row", new Uint8Array([2, 3, 255])] },
    ],
    session: { kind: 2 },
  });
  const write = await run(
    { sql: "INSERT INTO items VALUES (?)", params: ["value"] },
    "sealed",
  );
  assert.deepEqual(await write.json(), {
    success: true,
    meta: writeMeta,
    results: [],
  });
  assert.deepEqual(calls.at(-1).session, { kind: 3, bookmark: "sealed" });
  const count = calls.length;
  for (const params of [[{}], [[256]]]) {
    const invalid = await run({ sql: "SELECT ?", params });
    assert.equal(invalid.status, 400);
    assert.deepEqual(await invalid.json(), {
      success: false,
      error: "D1_TYPE_ERROR",
    });
  }
  assert.equal(calls.length, count);
  const failure = await run({ sql: "broken" });
  assert.deepEqual(await failure.json(), {
    success: false,
    error: "D1_INTERNAL_PROTOCOL_ERROR",
  });
});

test("D1 execute uses the existing sequential exec authority and stops after a failed line", async () => {
  const calls = [];
  class Transport extends Base {
    async executeStatements() {
      throw new Error("exec must not become a transactional batch");
    }
    async exec(sql) {
      calls.push(sql);
      if (sql === "broken")
        throw Object.assign(new Error("private SQL"), {
          stableCode: "D1_SQL_INVALID",
        });
      return { count: 1, duration: 2 };
    }
  }
  const response = await new Transport().fetch(
    new Request("http://d1/execute?resultsFormat=NONE", {
      method: "POST",
      body: JSON.stringify([
        { sql: "first" },
        { sql: "broken" },
        { sql: "never" },
      ]),
    }),
  );
  assert.deepEqual(calls, ["first", "broken"]);
  assert.deepEqual(await response.json(), [
    { success: true, meta: { duration: 2 }, results: [] },
    { success: false, error: "D1_SQL_INVALID" },
  ]);
});

test("D1 execute validates every input before mutation and rejects malformed summaries", async () => {
  let executions = 0;
  class Transport extends Base {
    async exec() {
      executions++;
      return { count: 1, duration: 0 };
    }
  }
  const request = (body) =>
    new Request("http://d1/execute?resultsFormat=NONE", {
      method: "POST",
      body: JSON.stringify(body),
    });
  const response = await new Transport().fetch(
    request([{ sql: "first" }, { sql: "bound", params: [1] }]),
  );
  assert.deepEqual(await response.json(), {
    success: false,
    error: "D1_TYPE_ERROR",
  });
  assert.equal(executions, 0);
  const empty = await new Transport().fetch(request([{ sql: "" }]));
  assert.deepEqual(await empty.json(), [
    { success: false, error: "No SQL statements detected." },
  ]);
  assert.equal(executions, 0);
  const prefix = await new Transport().fetch(
    request([{ sql: "first" }, { sql: "" }, { sql: "never" }]),
  );
  assert.deepEqual(await prefix.json(), [
    { success: true, meta: { duration: 0 }, results: [] },
    { success: false, error: "No SQL statements detected." },
  ]);
  assert.equal(executions, 1);
  for (const invalid of [
    null,
    { count: 0, duration: 0 },
    { count: 1, duration: Infinity },
  ]) {
    class Corrupt extends Base {
      async exec() {
        return invalid;
      }
    }
    const failed = await new Corrupt().fetch(request([{ sql: "statement" }]));
    assert.deepEqual(await failed.json(), [
      { success: false, error: "D1_INTERNAL_PROTOCOL_ERROR" },
    ]);
  }
});

test("D1 wire rejects malformed authority results before private values escape", async () => {
  for (const malformed of [
    null,
    { results: [], bookmark: null, stateVersion: 0 },
    {
      results: [{ columns: [42], rows: [["private"]], meta }],
      bookmark: null,
      stateVersion: 0,
    },
    {
      results: [{ columns: ["value"], rows: [[]], meta }],
      bookmark: null,
      stateVersion: 0,
    },
    {
      results: [{ columns: ["value"], rows: [[Number.NaN]], meta }],
      bookmark: null,
      stateVersion: 0,
    },
    {
      results: [
        {
          columns: ["value"],
          rows: [[1]],
          meta: { ...meta, privateToken: "secret" },
        },
      ],
      bookmark: null,
      stateVersion: 0,
    },
  ]) {
    class Corrupt extends Base {
      async executeStatements() {
        return malformed;
      }
    }
    const transport = new Corrupt();
    assert.equal(
      (await transport.query({ queries: [{ sql: "select 1" }] })).error.message,
      "D1_INTERNAL_PROTOCOL_ERROR",
    );
    const response = await transport.fetch(
      new Request("http://d1/query", {
        method: "POST",
        body: JSON.stringify({ sql: "select 1" }),
      }),
    );
    assert.deepEqual(await response.json(), {
      success: false,
      error: "D1_INTERNAL_PROTOCOL_ERROR",
    });
  }
});

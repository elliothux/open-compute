import assert from "node:assert/strict";
import test from "node:test";
import { deserialize, serialize } from "node:v8";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const envelope = moduleUrl(await compileRuntime("loader/envelope.ts"));
const imports = {
  "../assets/router.js": moduleUrl(
    "export function routeDefaultHttp() { throw Error('unused'); }",
  ),
  "../observability/collector.js": moduleUrl(
    "export function observedEntrypoint(stub) { return stub.getEntrypoint(); }",
  ),
  "../queues/metrics.js": moduleUrl(await compileRuntime("queues/metrics.ts")),
  "../services/facade.js": moduleUrl(
    "export const SERVICE_WEBSOCKET_HANDOFF_HEADER='x-handoff'; export function serviceWebSocketHandoffHandles() { throw Error('unused'); }",
  ),
  "./bindings.js": moduleUrl(
    "export function tenantEnv() { throw Error('unused'); } export function validationEnv() { throw Error('unused'); }",
  ),
  "./envelope.js": envelope,
  "./modules.js": moduleUrl(
    "export function modulesFor() { throw Error('unused'); }",
  ),
  "./shared.js":
    moduleUrl(`export const INTERNAL_HEADERS=[],TOKEN_HEADER='x-open-compute-internal-token';
  export const bindingError=(code)=>Object.assign(new Error(code),{stableCode:code});
  export const stableCode=(error)=>error?.stableCode;
  export const isRecord=(value)=>value!==null && typeof value==='object' && !Array.isArray(value);
  export async function resolveSnapshot(env) { ++env.reads; return {}; }
  export function doPolicy() { throw Error('unused'); }
  export function assembleOnce() { throw Error('unused'); }
  export function snapshotWorkerCode() { throw Error('unused'); }
  export function tenantGlobalOutbound() { throw Error('unused'); }
  export function tenantLoader(env) { return env.LOADER; }
  export async function loadTenantStub(env, _loaderKey, name, code) {
    return { stub: env.LOADER.get(name, code), release() {} };
  }`),
};
const { handleQueue } = await importRuntime("loader/dispatch.ts", imports);
const instance = "019c0000000070008000000000000001";
const key = `${instance}/019c0000-0000-7000-8000-000000000002/019c0000-0000-7000-8000-000000000003`;
const base64 = (bytes) => Buffer.from(bytes).toString("base64");
const text = (value) => new TextEncoder().encode(value);
function message(body = text('{"value":3}'), contentType = "json") {
  return {
    id: "one",
    timestampMs: 1000,
    attempts: 1,
    contentType,
    bodyBase64: base64(body),
  };
}
function payload() {
  return {
    queueName: "events",
    messages: [message()],
    metadata: {
      metrics: {
        backlogCount: 9,
        backlogBytes: 99,
        oldestMessageTimestampMs: 500,
      },
    },
  };
}
function fixture(
  queue = async () => ({
    outcome: "ok",
    ackAll: false,
    retryBatch: { retry: false },
    explicitAcks: [],
    retryMessages: [],
  }),
) {
  const env = {
    QUEUE_WIRE_CODEC: { decodeV8: deserialize },
    reads: 0,
    loads: 0,
    calls: [],
    LOADER: {
      get(runtimeKey) {
        ++env.loads;
        env.runtimeKey = runtimeKey;
        return {
          getEntrypoint() {
            return {
              async queue(...args) {
                env.calls.push(args);
                return queue(...args);
              },
            };
          },
        };
      },
    },
  };
  const dispatchRaw = (body, headerOverrides = {}) =>
    handleQueue(
      new Request("https://loader.invalid/queue", {
        method: "POST",
        headers: {
          "x-open-compute-loader-key": key,
          "x-open-compute-instance-id": instance,
          "x-open-compute-worker-code-sha256": "a".repeat(64),
          "x-open-compute-route-generation": "1",
          ...headerOverrides,
        },
        body,
      }),
      env,
      {},
    );
  const dispatch = (value, headerOverrides = {}) =>
    dispatchRaw(JSON.stringify(value), headerOverrides);
  return { env, dispatch, dispatchRaw };
}
async function invalid(value) {
  const { env, dispatch } = fixture();
  const response = await dispatch(value);
  assert.equal(response.status, 422);
  assert.equal((await response.json()).error.code, "QUEUE_DISPOSITION_INVALID");
  assert.equal(env.reads, 0);
  assert.equal(env.loads, 0);
  assert.equal(env.calls.length, 0);
}

test("Queue consumer restores current durable bodies and metadata before native dispatch", async () => {
  const current = payload();
  current.messages.push({
    ...message(text("plain"), "text"),
    id: "two",
    attempts: 2,
  });
  current.messages.push({
    ...message(new Uint8Array([1, 2, 255]), "bytes"),
    id: "three",
    attempts: 3,
  });
  current.messages.push({
    ...message(
      serialize({ when: new Date(2000), mapping: new Map([["key", "value"]]) }),
      "v8",
    ),
    id: "four",
    attempts: 4,
  });
  const { env, dispatch } = fixture();
  const response = await dispatch(current);
  assert.equal(response.status, 200);
  assert.equal((await response.json()).outcome, "ok");
  assert.equal(env.reads, 1);
  assert.equal(env.loads, 1);
  assert.equal(env.calls.length, 1);
  const [queue, messages, metadata] = env.calls[0];
  assert.equal(queue, "events");
  assert.deepEqual(
    messages.map((m) => m.id),
    ["one", "two", "three", "four"],
  );
  assert.deepEqual(
    messages.map((m) => m.attempts),
    [1, 2, 3, 4],
  );
  assert.ok(
    messages.every(
      (m) => m.timestamp instanceof Date && m.timestamp.getTime() === 1000,
    ),
  );
  assert.deepEqual(messages[0].body, { value: 3 });
  assert.equal(messages[1].body, "plain");
  assert.deepEqual(messages[2].body, new Uint8Array([1, 2, 255]));
  assert.deepEqual(messages[3].body, {
    when: new Date(2000),
    mapping: new Map([["key", "value"]]),
  });
  assert.deepEqual(metadata, {
    metrics: {
      backlogCount: 9,
      backlogBytes: 99,
      oldestMessageTimestamp: new Date(500),
    },
  });
});

test("Queue consumer malformed body formats fail closed before source resolution", async () => {
  for (const bad of [
    message(text("{")),
    message(new Uint8Array([255]), "text"),
    message(new Uint8Array([255]), "v8"),
    message(text("x"), "unknown"),
    { ...message(), bodyBase64: "!" },
    { ...message(), bodyBase64: "Zg" },
    { ...message(), bodyBase64: " Zg==" },
    { ...message(), bodyBase64: "Zh==" },
  ]) {
    const current = payload();
    current.messages = [bad];
    await invalid(current);
  }
});

test("Queue consumer metadata rejects malformed counters and invalid dates before loading", async () => {
  for (const metadata of [
    null,
    [],
    {},
    { metrics: [] },
    { metrics: { backlogCount: -1, backlogBytes: 0 } },
    { metrics: { backlogCount: 0, backlogBytes: 0.5 } },
    {
      metrics: {
        backlogCount: 0,
        backlogBytes: 0,
        oldestMessageTimestampMs: "private-token",
      },
    },
    {
      metrics: {
        backlogCount: 0,
        backlogBytes: 0,
        oldestMessageTimestampMs: Number.MAX_SAFE_INTEGER,
      },
    },
  ]) {
    await invalid({ ...payload(), metadata });
  }
  for (const oldest of [undefined, null, 0]) {
    const { env, dispatch } = fixture();
    const current = payload();
    current.metadata.metrics.oldestMessageTimestampMs = oldest;
    assert.equal((await dispatch(current)).status, 200);
    assert.deepEqual(env.calls[0][2], {
      metrics: { backlogCount: 9, backlogBytes: 99 },
    });
  }
  const { env, dispatch } = fixture();
  const current = payload();
  delete current.metadata;
  assert.equal((await dispatch(current)).status, 200);
  assert.deepEqual(env.calls[0][2], {
    metrics: { backlogCount: 0, backlogBytes: 0 },
  });
});

test("Queue consumer message envelope and batch budgets are checked before loading", async () => {
  for (const change of [
    { timestampMs: -1 },
    { timestampMs: 0.5 },
    { timestampMs: Number.MAX_SAFE_INTEGER },
    { attempts: 0 },
    { attempts: 102 },
    { attempts: 1.5 },
    { id: 3 },
  ]) {
    await invalid({ ...payload(), messages: [{ ...message(), ...change }] });
  }
  await invalid({ ...payload(), messages: [] });
  await invalid({
    ...payload(),
    messages: Array.from({ length: 101 }, () => message()),
  });
  await invalid({
    ...payload(),
    messages: [message(new Uint8Array(128001), "bytes")],
  });
  await invalid({
    ...payload(),
    messages: [0, 1, 2].map((i) => ({
      ...message(new Uint8Array(100000), "bytes"),
      id: String(i),
    })),
  });
  await invalid({ ...payload(), queueName: "" });
});

test("Queue consumer preserves native disposition results and sanitizes entrypoint exceptions", async () => {
  const result = {
    outcome: "exception",
    ackAll: false,
    retryBatch: { retry: true, delaySeconds: 7 },
    explicitAcks: ["one"],
    retryMessages: [],
  };
  const { dispatch } = fixture(async () => result);
  assert.deepEqual(await (await dispatch(payload())).json(), result);
  for (const cause of [
    new Error("Traceback /private/path token=private-secret"),
    Object.assign(new Error("private-secret"), {
      stableCode: "VERSION_NOT_READY",
    }),
  ]) {
    const current = fixture(async () => {
      throw cause;
    });
    const response = await current.dispatch(payload());
    assert.equal(response.status, cause.stableCode ? 422 : 500);
    const output = await response.json();
    assert.equal(
      output.error.code,
      cause.stableCode ?? "QUEUE_CUSTOM_EVENT_UNSUPPORTED",
    );
    assert.doesNotMatch(
      JSON.stringify(output),
      /private-secret|Traceback|private\/path/,
    );
  }
});

test("Queue consumer rejects malformed JSON request bodies before resolving source", async () => {
  const { env, dispatchRaw } = fixture();
  const response = await dispatchRaw("{");
  assert.equal(response.status, 422);
  assert.equal((await response.json()).error.code, "QUEUE_DISPOSITION_INVALID");
  assert.equal(env.reads, 0);
  assert.equal(env.loads, 0);
  assert.equal(env.calls.length, 0);
});

test("Queue consumer decodes large native V8 bodies and enforces decimal wire budgets", async () => {
  const value = "你".repeat(50_000);
  const { env, dispatch } = fixture();
  const current = { ...payload(), messages: [message(serialize(value), "v8")] };
  assert.equal((await dispatch(current)).status, 200);
  assert.equal(env.calls[0][1][0].body, value);
  current.messages = [message(serialize("a".repeat(127994)), "v8")];
  assert.equal((await dispatch(current)).status, 200);
  assert.equal(env.calls[1][1][0].body.length, 127994);
  await invalid({
    ...payload(),
    messages: [message(serialize(undefined), "v8")],
  });
  await invalid({
    ...payload(),
    messages: [message(serialize("a".repeat(127995)), "v8")],
  });
  const exact = {
    ...payload(),
    messages: [
      message(new Uint8Array(128000), "bytes"),
      { ...message(new Uint8Array(128000), "bytes"), id: "two" },
    ],
  };
  assert.equal((await dispatch(exact)).status, 200);
  await invalid({
    ...exact,
    messages: [
      ...exact.messages,
      { ...message(new Uint8Array(1), "bytes"), id: "three" },
    ],
  });
});

import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const { authorityFromHeaders, cancelOrderedOperation, ordered } =
  await importRuntime("durable-objects/host-protocol.ts", {
    "./errors.js": moduleUrl(await compileRuntime("durable-objects/errors.ts")),
    "./identity.js": moduleUrl(
      await compileRuntime("durable-objects/identity.ts"),
    ),
    "../loader/shared.js": moduleUrl(
      "export const bindingError = (code) => new Error(code);",
    ),
  });

test("DO host authority accepts only the canonical instance ID", () => {
  const headers = new Headers({
    "x-open-compute-instance-id": "019c0000000070008000000000000001",
    "x-open-compute-worker-id": "019c0000-0000-7000-8000-000000000002",
    "x-open-compute-version-id": "019c0000-0000-7000-8000-000000000003",
    "x-open-compute-worker-code-sha256": "a".repeat(64),
    "x-open-compute-object-id": "b".repeat(64),
    "x-open-compute-namespace-resource-id":
      "019c0000-0000-7000-8000-000000000004",
    "x-open-compute-class-name": "Object",
    "x-open-compute-route-generation": "1",
    "x-open-compute-object-generation": "1",
  });
  assert.equal(
    authorityFromHeaders(headers).instanceId,
    "019c0000000070008000000000000001",
  );
  headers.set(
    "x-open-compute-instance-id",
    "019c0000-0000-7000-8000-000000000001",
  );
  assert.throws(
    () => authorityFromHeaders(headers),
    /DO_INTERNAL_PROTOCOL_ERROR/,
  );
});

test("RPC admission blocks following events until startup, while allowing in-flight overlap", async () => {
  const states = new Map();
  const channelId = "a".repeat(32);
  const events = [];
  let acknowledge;
  let complete;
  const held = new Promise((resolve) => {
    complete = resolve;
  });
  const first = ordered(
    states,
    { channelId, sequence: 0 },
    async (started) => {
      acknowledge = started;
      events.push("rpc:admitting");
      await held;
      events.push("rpc:complete");
    },
    true,
  );
  const second = ordered(states, { channelId, sequence: 1 }, async () => {
    events.push("fetch:start");
  });
  const third = ordered(states, { channelId, sequence: 2 }, async () => {
    events.push("connect:start");
  });
  await Promise.resolve();
  assert.deepEqual(events, ["rpc:admitting"]);
  acknowledge();
  acknowledge();
  await Promise.all([second, third]);
  assert.deepEqual(events, ["rpc:admitting", "fetch:start", "connect:start"]);
  complete();
  await first;
  assert.equal(events.at(-1), "rpc:complete");
});

test("a rejected RPC startup releases its ordered successor", async () => {
  const states = new Map();
  const channelId = "b".repeat(32);
  let rejectStartup;
  const startup = new Promise((_resolve, reject) => {
    rejectStartup = reject;
  });
  const first = ordered(
    states,
    { channelId, sequence: 0 },
    () => startup,
    true,
  );
  const failed = assert.rejects(first, /startup-failed/);
  const second = ordered(
    states,
    { channelId, sequence: 1 },
    async () => "next",
  );
  rejectStartup(new Error("startup-failed"));
  await failed;
  assert.equal(await second, "next");
});

test("cancelOrderedOperation skips a queued sequence without blocking the channel", async () => {
  const states = new Map();
  const channelId = "a".repeat(32);
  let ran = false;
  const second = ordered(states, { channelId, sequence: 1 }, async () => {
    ran = true;
  });
  const cancelled = assert.rejects(second, /DO_RUNTIME_EXCEPTION/);
  cancelOrderedOperation(states, { channelId, sequence: 1 });
  await cancelled;
  assert.equal(
    await ordered(states, { channelId, sequence: 0 }, async () => "first"),
    "first",
  );
  const third = await ordered(
    states,
    { channelId, sequence: 2 },
    async () => "third",
  );
  assert.equal(third, "third");
  assert.equal(ran, false);
  assert.equal(states.get(channelId).pending.size, 0);
});

test("cancellation before the first request advances an unseen channel", async () => {
  const states = new Map();
  const channelId = "c".repeat(32);
  cancelOrderedOperation(states, { channelId, sequence: 0 });
  cancelOrderedOperation(states, { channelId, sequence: 0 });
  assert.equal(
    await ordered(states, { channelId, sequence: 1 }, async () => "next"),
    "next",
  );
  assert.throws(
    () => ordered(states, { channelId, sequence: 0 }, async () => "stale"),
    /DO_RUNTIME_EXCEPTION/,
  );
});

test("future cancellations remain bounded and cannot execute tenant work", async () => {
  const states = new Map();
  const channelId = "d".repeat(32);
  for (let sequence = 1; sequence <= 256; sequence += 1)
    cancelOrderedOperation(states, { channelId, sequence });
  assert.throws(
    () => cancelOrderedOperation(states, { channelId, sequence: 257 }),
    /DO_STORAGE_LIMIT/,
  );
  cancelOrderedOperation(states, { channelId, sequence: 0 });
  assert.equal(
    await ordered(states, { channelId, sequence: 257 }, async () => "next"),
    "next",
  );
});

test("ordered rejects duplicate pending sequences with DO_RUNTIME_EXCEPTION", async () => {
  const states = new Map();
  const channelId = "b".repeat(32);
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  void ordered(states, { channelId, sequence: 0 }, async () => {
    await gate;
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.throws(
    () => ordered(states, { channelId, sequence: 0 }, async () => undefined),
    /DO_RUNTIME_EXCEPTION/,
  );
  release();
});

test("a full ordering buffer still admits the next sequence so pending work can drain", async () => {
  const states = new Map();
  const channelId = "e".repeat(32);
  const pending = Array.from({ length: 256 }, (_, index) =>
    ordered(states, { channelId, sequence: index + 1 }, async () => index + 1),
  );
  assert.throws(
    () => ordered(states, { channelId, sequence: 257 }, async () => 257),
    /DO_STORAGE_LIMIT/,
  );
  assert.equal(
    await ordered(states, { channelId, sequence: 0 }, async () => 0),
    0,
  );
  assert.deepEqual(
    await Promise.all(pending),
    Array.from({ length: 256 }, (_, index) => index + 1),
  );
  assert.equal(states.get(channelId).pending.size, 0);
});

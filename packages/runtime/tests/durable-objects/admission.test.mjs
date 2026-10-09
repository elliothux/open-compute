import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const timers = new Set();
globalThis.scheduler = {
  wait: (_, { signal }) =>
    new Promise((resolve, reject) => {
      const finish = () => {
        timers.delete(finish);
        resolve();
      };
      timers.add(finish);
      signal.addEventListener(
        "abort",
        () => {
          timers.delete(finish);
          reject(signal.reason);
        },
        { once: true },
      );
    }),
};
const { admitted } = await importRuntime("durable-objects/admission.ts", {
  "../loader/shared.js": moduleUrl(`
    export const doPolicy = (env) => env;
    export const bindingError = (code) => Object.assign(new Error(code), { stableCode: code });
  `),
});
const policy = { maxInFlightDispatches: 1, dispatchTimeoutMs: 10 };

test("overload never queues tenant work, and timeout retains the running slot until settlement", async () => {
  let release;
  const running = admitted(
    policy,
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  await Promise.resolve();
  let ran = false;
  const rejectedWork = () => {
    ran = true;
    return Promise.resolve();
  };
  assert.throws(() => admitted(policy, rejectedWork), {
    stableCode: "DO_STORAGE_LIMIT",
  });
  const timeout = assert.rejects(running, {
    stableCode: "DO_DISPATCH_TIMEOUT",
  });
  timers.values().next().value();
  await timeout;
  assert.throws(() => admitted(policy, rejectedWork), {
    stableCode: "DO_STORAGE_LIMIT",
  });
  release();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(ran, false);
  assert.equal(await admitted(policy, async () => "recovered"), "recovered");
  assert.equal(timers.size, 0);
});

test("rejected tenant work releases its admission slot", async () => {
  await assert.rejects(
    admitted(policy, async () => {
      throw new Error("tenant failure");
    }),
    /tenant failure/,
  );
  assert.equal(await admitted(policy, async () => "recovered"), "recovered");
  assert.equal(timers.size, 0);
});

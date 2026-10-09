import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { createFetchAdmission } = await importRuntime(
  "durable-objects/fetch-admission.ts",
  {},
);

test("fetch admission hides its private token, is single-use, and releases abandoned callbacks", async () => {
  const prior = globalThis.scheduler;
  const timers = [];
  const tasks = [];
  globalThis.scheduler = {
    wait(delay, { signal }) {
      assert.equal(delay, 30_000);
      return new Promise((resolve) => {
        timers.push(resolve);
        signal.addEventListener("abort", resolve, { once: true });
      });
    },
  };
  try {
    const admission = createFetchAdmission({
      waitUntil(task) {
        tasks.push(task);
      },
    });
    const token = "11111111-1111-4111-8111-111111111111";
    let started = 0;
    let disposed = 0;
    const callback = Object.assign(
      async () => {
        throw new Error("borrowed callback expired");
      },
      {
        dup() {
          let closed = false;
          return Object.assign(
            async () => {
              assert.equal(closed, false);
              started += 1;
            },
            {
              [Symbol.dispose]() {
                assert.equal(closed, false);
                closed = true;
                disposed += 1;
              },
            },
          );
        },
      },
    );
    admission.prepare(token, callback, 30_000);
    assert.throws(
      () => admission.prepare(token, callback, 30_000),
      /DO_INTERNAL_PROTOCOL_ERROR/,
    );
    const request = () =>
      new Request("https://object.invalid/", {
        method: "POST",
        body: "tenant-body",
        headers: {
          "x-open-compute-fetch-admission": token,
          "x-tenant": "kept",
        },
      });
    const args = [request()];
    const acknowledge = admission.start(args);
    assert.equal(args[0].headers.has("x-open-compute-fetch-admission"), false);
    assert.equal(args[0].headers.get("x-tenant"), "kept");
    assert.equal(await args[0].text(), "tenant-body");
    assert.equal(started, 0);
    await acknowledge();
    assert.equal(started, 1);
    assert.equal(disposed, 1);
    assert.throws(() => admission.start([request()]), /DO_RUNTIME_EXCEPTION/);
    await tasks[0];
    admission.prepare(token, callback, 30_000);
    timers.at(-1)();
    await tasks.at(-1);
    assert.equal(disposed, 2);
    assert.throws(() => admission.start([request()]), /DO_RUNTIME_EXCEPTION/);
    assert.equal(
      admission.start([new Request("https://object.invalid/")]),
      undefined,
    );
    assert.equal(admission.start([]), undefined);
    assert.throws(
      () => admission.prepare("invalid", async () => {}, 30_000),
      /DO_INTERNAL_PROTOCOL_ERROR/,
    );
  } finally {
    if (prior === undefined) delete globalThis.scheduler;
    else globalThis.scheduler = prior;
  }
});

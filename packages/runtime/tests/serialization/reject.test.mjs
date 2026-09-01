import assert from "node:assert/strict";
import test from "node:test";
import { codec, encode, profiles } from "./load.mjs";

function rejects(profile, value, code = profile === "queue-v8"
  ? "QUEUE_V8_UNSUPPORTED" : "WORKFLOW_SERIALIZATION_UNSUPPORTED") {
  const name = profile === "queue-v8" ? "TypeError" : "Error";
  assert.throws(() => encode(value, profile), { name, message: code });
  assert.equal(codec.durableValueErrorCode(
    (() => { try { encode(value, profile); } catch (error) { return error; } })(),
    profile,
  ), code);
}

test("unsupported values fail closed with profile-specific codes and are not coerced", () => {
  class Box {}
  class SubArray extends Array {}
  class SubMap extends Map {}
  const fn = () => "nope";
  for (const profile of profiles) {
    for (const value of [
      Symbol("x"), fn, { [Symbol("s")]: 1 }, { get x() { return 1; } },
      { set x(value) { void value; } }, new Box(), new SubArray(1), new SubMap([[1, 2]]),
      new Number(1), new String("x"), new Boolean(true), Object(1n),
      Promise.resolve(1), new WeakMap(), new WeakSet(), new Request("https://example.com/"),
      new Response("x"), new Headers({ a: "b" }), new ReadableStream(), new WritableStream(),
      new TransformStream(), new URL("https://example.com/"),
    ]) {
      rejects(profile, value);
    }
    const accessor = {};
    Object.defineProperty(accessor, "x", { get: fn, enumerable: true });
    rejects(profile, accessor);
    const method = { a: 1, toJSON: fn };
    rejects(profile, method);
    const arrayMethod = [1];
    arrayMethod.push(fn);
    rejects(profile, arrayMethod);
    rejects(profile, new Map([[fn, 1]]));
    rejects(profile, new Set([Symbol("x")]));
    rejects(profile, { nested: { fn } });
  }
});

test("unsafe buffers, transferables, and host streams are rejected before persistence", () => {
  for (const profile of profiles) {
    if (typeof SharedArrayBuffer === "function") {
      try {
        rejects(profile, new SharedArrayBuffer(8));
        rejects(profile, new Uint8Array(new SharedArrayBuffer(8)));
      } catch (error) {
        if (!(error instanceof Error) || !/SharedArrayBuffer|secure context/.test(error.message)) throw error;
      }
    }
    const resizable = new ArrayBuffer(8, { maxByteLength: 16 });
    if (resizable.resizable) {
      rejects(profile, resizable);
      rejects(profile, new Uint8Array(resizable));
      rejects(profile, new DataView(resizable));
    }
    const buffer = new ArrayBuffer(8);
    if (typeof buffer.transfer === "function") {
      const detached = buffer.transfer();
      void detached;
      rejects(profile, buffer);
      try { rejects(profile, new Uint8Array(buffer)); }
      catch { rejects(profile, buffer); }
    }
  }
});

test("over-depth and over-node bounds use the profile too-large code", () => {
  for (const profile of profiles) {
    const limits = codec.durableValueLimits(profile);
    let deep = null;
    for (let index = 0; index < limits.maxDepth; index++) deep = [deep];
    encode(deep, profile);
    assert.throws(() => encode([deep], profile), {
      message: profile === "queue-v8" ? "QUEUE_V8_TOO_LARGE" : "WORKFLOW_RESULT_TOO_LARGE",
    });
    const nodes = Array.from({ length: 30_000 }, () => ({}));
    if (profile === "queue-v8") {
      assert.throws(() => encode(nodes, profile), { message: "QUEUE_V8_TOO_LARGE" });
    } else {
      encode(nodes, profile);
    }
  }
});

test("queue-v8 is bounded below the workflow limit", () => {
  const payload = "x".repeat(200_000);
  assert.throws(() => encode(payload, "queue-v8"), { name: "TypeError", message: "QUEUE_V8_TOO_LARGE" });
  encode(payload, "workflow");
});

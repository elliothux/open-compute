import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "./compiled-runtime.mjs";

const { privateWeakMap } = await importRuntime("private-weak-map.ts");
const { KVNamespace } = await importRuntime("kv/facade.ts");

test("private capability maps ignore tenant prototype edits", () => {
  const map = privateWeakMap();
  const key = {};
  const raw = { secret: true };
  const originalGet = WeakMap.prototype.get;
  const originalSet = WeakMap.prototype.set;
  let intercepted = false;
  try {
    WeakMap.prototype.get = () => raw;
    WeakMap.prototype.set = () => {
      intercepted = true;
      throw new Error("raw capability intercepted");
    };
    assert.equal(map.get(key), undefined);
    map.set(key, raw);
    assert.equal(map.get(key), raw);
    const namespace = new KVNamespace(raw);
    assert.equal(namespace instanceof KVNamespace, true);
    assert.equal(intercepted, false);
  } finally {
    WeakMap.prototype.get = originalGet;
    WeakMap.prototype.set = originalSet;
  }
});

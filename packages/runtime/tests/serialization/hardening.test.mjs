import assert from "node:assert/strict";
import test from "node:test";
import { decode, encode, profiles, roundTrip } from "./load.mjs";

test("structured-clone hardening: getters, toJSON, and iterators are not invoked", () => {
  for (const profile of profiles) {
    let getterRan = false;
    const getter = {
      get x() {
        getterRan = true;
        return 1;
      },
    };
    assert.throws(() => encode(getter, profile));
    assert.equal(getterRan, false);

    let toJsonRan = false;
    const json = {
      a: 1,
      toJSON() {
        toJsonRan = true;
        return { a: 2 };
      },
    };
    assert.throws(() => encode(json, profile));
    assert.equal(toJsonRan, false);

    let hiddenJsonRan = false;
    const hidden = { a: 1 };
    Object.defineProperty(hidden, "toJSON", {
      value() {
        hiddenJsonRan = true;
        return 0;
      },
    });
    assert.deepEqual(roundTrip(hidden, profile), { a: 1 });
    assert.equal(hiddenJsonRan, false);

    let iterated = false;
    const array = [1, 2, 3];
    Object.defineProperty(array, Symbol.iterator, {
      value: function* () {
        iterated = true;
        yield 9;
      },
    });
    assert.deepEqual(roundTrip(array, profile), [1, 2, 3]);
    assert.equal(iterated, false);

    const map = new Map([[1, 2]]);
    Object.defineProperty(map, Symbol.iterator, {
      value: function* () {
        iterated = true;
        yield [3, 4];
      },
    });
    assert.deepEqual([...roundTrip(map, profile)], [[1, 2]]);
    assert.equal(iterated, false);

    const set = new Set(["a"]);
    Object.defineProperty(set, "forEach", {
      value() { iterated = true; },
    });
    assert.deepEqual([...roundTrip(set, profile)], ["a"]);
    assert.equal(iterated, false);
  }
});

test("structured-clone hardening: prototype setters are not invoked on decode", () => {
  const marker = Symbol("polluted");
  const descriptor = {
    set() { Object.defineProperty(Object.prototype, marker, { value: true, configurable: true }); },
    configurable: true,
  };
  Object.defineProperty(Object.prototype, "injected", descriptor);
  try {
    const decoded = decode(encode({ injected: 1 }, "workflow"), "workflow");
    assert.equal(decoded.injected, 1);
    assert.equal(Object.prototype[marker], undefined);
    assert.equal(Object.getPrototypeOf(decoded), Object.prototype);
  } finally {
    delete Object.prototype.injected;
    delete Object.prototype[marker];
  }
});

test("structured-clone hardening: sparse arrays keep holes and do not use array extras as length", () => {
  const value = ["a"];
  value.length = 5;
  value[4] = "z";
  value.extra = 3;
  const decoded = roundTrip(value, "queue-v8");
  assert.equal(decoded.length, 5);
  assert.equal(0 in decoded, true);
  assert.equal(1 in decoded, false);
  assert.equal(2 in decoded, false);
  assert.equal(3 in decoded, false);
  assert.equal(decoded[4], "z");
  assert.equal(decoded.extra, 3);
});

test("RPC stubs and class instances are rejected rather than narrowed", () => {
  const stub = Object.create({ dup() { return this; } });
  stub.dup = stub.dup;
  for (const profile of profiles) {
    assert.throws(() => encode(stub, profile));
    assert.throws(() => encode(new (class RpcTarget {})(), profile));
    assert.throws(() => encode(new (class DurableObject {})(), profile));
  }
});

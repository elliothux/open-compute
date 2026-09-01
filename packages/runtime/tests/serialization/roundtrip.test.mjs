import assert from "node:assert/strict";
import test from "node:test";
import { assertRoundTrip, codec, decode, encode, graphEqual, profiles, roundTrip } from "./load.mjs";

const typed = [
  Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array,
  Float32Array, Float64Array, BigInt64Array, BigUint64Array,
];

test("profiles round-trip the declared durable subset", () => {
  for (const profile of profiles) {
    const values = [
      null, undefined, true, false, 0, -0, 1, -1, 1.5, Number.NaN, Infinity, -Infinity,
      0n, 1n, -1n, 255n, 256n, -(2n ** 64n), 2n ** 80n, "", "ascii", "unicode 𐀀", "\uD800",
      "\uD800\uDC00", "\n\t", [], [1, , undefined, 4], { z: 1, a: 2 }, Object.create(null),
      new Date(0), new Date(NaN), new Date(-8640000000000000), /foo/gi, new RegExp("a+", "ms"),
      new Error("e"), new TypeError("t"), new RangeError("r"), new Uint8Array([1, 2, 3]),
      new ArrayBuffer(0), new ArrayBuffer(4), new Map(), new Set(), new Map([[1, "a"], [2, "b"]]),
      new Set(["a", 1n, null]),
    ];
    for (const value of values) assertRoundTrip(assert, value, profile);
    const object = Object.create(null);
    object.ok = true;
    assert.equal(Object.getPrototypeOf(roundTrip(object, profile)), null);
    assert.equal(Object.getPrototypeOf(roundTrip({ a: 1 }, profile)), Object.prototype);
    const holes = [1];
    holes.length = 4;
    holes.named = "extra";
    holes[3] = 9;
    const decodedHoles = assertRoundTrip(assert, holes, profile);
    assert.equal(1 in decodedHoles, false);
    assert.equal(2 in decodedHoles, false);
    assert.equal(decodedHoles.named, "extra");
    const regexp = /x/gy;
    regexp.lastIndex = 4;
    assert.equal(roundTrip(regexp, profile).lastIndex, 4);
    const invalid = new Date(NaN);
    assert.ok(Number.isNaN(roundTrip(invalid, profile).getTime()));
    assert.ok(Object.is(roundTrip(-0, profile), -0));
    assert.ok(Number.isNaN(roundTrip(Number.NaN, profile)));
  }
});

test("cycles and shared references keep object identity", () => {
  for (const profile of profiles) {
    const cycle = { name: "root" };
    cycle.self = cycle;
    const decodedCycle = roundTrip(cycle, profile);
    assert.equal(decodedCycle.self, decodedCycle);
    assert.equal(decodedCycle.name, "root");
    const shared = { n: 1 };
    const diamond = { left: shared, right: shared };
    const decodedDiamond = roundTrip(diamond, profile);
    assert.equal(decodedDiamond.left, decodedDiamond.right);
    decodedDiamond.left.n = 9;
    assert.equal(decodedDiamond.right.n, 9);
    const array = [];
    array.push(array, { array });
    const decodedArray = roundTrip(array, profile);
    assert.equal(decodedArray[0], decodedArray);
    assert.equal(decodedArray[1].array, decodedArray);
    const map = new Map();
    map.set(map, map);
    const decodedMap = roundTrip(map, profile);
    assert.equal([...decodedMap.keys()][0], decodedMap);
    assert.equal(decodedMap.get(decodedMap), decodedMap);
    const set = new Set();
    set.add(set);
    assert.equal([...roundTrip(set, profile)][0] instanceof Set, true);
    const nested = { a: { b: { c: 1 } } };
    nested.a.b.back = nested;
    assert.equal(roundTrip(nested, profile).a.b.back.a.b.c, 1);
  }
});

test("typed-array classes preserve type, offset, contents, and shared buffers", () => {
  for (const profile of profiles) {
    const buffer = new ArrayBuffer(64);
    new Uint8Array(buffer).set(Array.from({ length: 64 }, (_, index) => index));
    for (const Ctor of typed) {
      const view = new Ctor(buffer, Ctor.BYTES_PER_ELEMENT, 2);
      const decoded = roundTrip({ view, buffer }, profile);
      assert.equal(decoded.view.constructor, Ctor);
      assert.equal(decoded.view.byteOffset, Ctor.BYTES_PER_ELEMENT);
      assert.equal(decoded.view.length, 2);
      assert.equal(decoded.view.buffer, decoded.buffer);
      assert.deepEqual(Array.from(new Uint8Array(decoded.view.buffer)), [...new Uint8Array(buffer)]);
      const sample = Ctor === BigInt64Array || Ctor === BigUint64Array
        ? new Ctor([1n, 2n, 3n]) : new Ctor([1, 2, 3]);
      assertRoundTrip(assert, sample, profile);
    }
    const view = new DataView(buffer, 3, 5);
    const decodedView = roundTrip({ view, buffer }, profile);
    assert.equal(decodedView.view instanceof DataView, true);
    assert.equal(decodedView.view.byteOffset, 3);
    assert.equal(decodedView.view.byteLength, 5);
    assert.equal(decodedView.view.buffer, decodedView.buffer);
    const bigintView = new BigInt64Array([1n, -2n, 3n]);
    assert.deepEqual([...roundTrip(bigintView, profile)], [1n, -2n, 3n]);
  }
});

test("Map and Set preserve insertion order including object keys", () => {
  for (const profile of profiles) {
    const key = { k: 1 };
    const map = new Map([["z", 1], [key, 2], [0n, 3], ["z2", key]]);
    const decoded = roundTrip(map, profile);
    assert.deepEqual([...decoded.keys()].map((item) => typeof item), ["string", "object", "bigint", "string"]);
    assert.equal([...decoded.values()][3], [...decoded.keys()][1]);
    const set = new Set(["b", key, "a", key]);
    assert.deepEqual([...roundTrip(set, profile)].map((item) => typeof item === "object" ? "object" : item),
      ["b", "object", "a"]);
  }
});

test("Error and DOMException keep safe fields without copying stack", () => {
  for (const profile of profiles) {
    const cause = new Error("inner");
    const error = new TypeError("outer", { cause });
    error.stack = "secret-stack";
    const decoded = roundTrip(error, profile);
    assert.equal(decoded.name, "TypeError");
    assert.equal(decoded.message, "outer");
    assert.equal(decoded.cause.message, "inner");
    assert.notEqual(decoded.stack, "secret-stack");
    const aggregate = new AggregateError([new Error("a"), new RangeError("b")], "many");
    const decodedAggregate = roundTrip(aggregate, profile);
    assert.equal(decodedAggregate.errors[0].message, "a");
    assert.equal(decodedAggregate.errors[1].name, "RangeError");
    if (typeof DOMException === "function") {
      const exception = new DOMException("denied", "NotAllowedError");
      const decodedException = roundTrip(exception, profile);
      assert.equal(decodedException.name, "NotAllowedError");
      assert.equal(decodedException.message, "denied");
    }
    const loop = new Error("self");
    loop.cause = loop;
    const decodedLoop = roundTrip(loop, profile);
    assert.equal(decodedLoop.cause, decodedLoop);
  }
});

test("identical graphs encode deterministically and count size from the header", () => {
  for (const profile of profiles) {
    const value = { a: [1, -0, 2n], b: new Uint8Array([9, 8]), c: new Date(1) };
    value.a.push(value);
    const first = encode(value, profile);
    const second = encode(value, profile);
    assert.deepEqual(first, second);
    assert.deepEqual(first, encode(decode(first, profile), profile));
    assert.equal(encode(null, profile).byteLength, 7);
    assert.equal(encode("a", profile).byteLength, 12);
    assert.equal(first[0], 0x4f);
    assert.equal(first[1], 0x43);
    assert.equal(first[2], 0x44);
    assert.equal(first[3], 0x56);
    assert.equal(first[4], codec.DURABLE_VALUE_SCHEMA);
    assert.equal(first[5], codec.DURABLE_VALUE_PROFILE_ID[profile]);
    const limit = codec.durableValueLimits(profile).maxBytes;
    const maxString = "x".repeat(limit - 11);
    assert.equal(encode(maxString, profile).byteLength, limit);
    assert.throws(() => encode(`${maxString}y`, profile), { message: profile === "queue-v8"
      ? "QUEUE_V8_TOO_LARGE" : "WORKFLOW_RESULT_TOO_LARGE" });
  }
});

test("property-style random acyclic graphs round-trip", () => {
  function leaf(seed) {
    const options = [null, undefined, seed % 2 === 0, seed, -0, seed + 0.5, BigInt(seed), `s${seed}`,
      new Date(seed), new Uint8Array([seed & 255, 1]).buffer];
    return options[seed % options.length];
  }
  function build(seed, depth) {
    if (depth === 0) return leaf(seed);
    if (seed % 5 === 0) return [build(seed + 1, depth - 1), leaf(seed), build(seed + 3, depth - 1)];
    if (seed % 5 === 1) return { a: build(seed + 2, depth - 1), b: leaf(seed) };
    if (seed % 5 === 2) return new Map([["k", build(seed + 4, depth - 1)], [leaf(seed), 1]]);
    if (seed % 5 === 3) return new Set([build(seed + 5, depth - 1), leaf(seed)]);
    return new Uint16Array([seed & 0xffff, 7]);
  }
  for (let seed = 0; seed < 80; seed++) {
    const value = build(seed, 3);
    const decoded = roundTrip(value, seed % 2 === 0 ? "workflow" : "queue-v8");
    assert.ok(graphEqual(value, decoded), String(seed));
  }
});

test("own __proto__ keys round-trip as data without changing the prototype", () => {
  const value = { safe: true };
  Object.defineProperty(value, "__proto__", {
    value: { polluted: true }, enumerable: true, writable: true, configurable: true,
  });
  const decoded = roundTrip(value, "workflow");
  assert.equal(Object.getPrototypeOf(decoded), Object.prototype);
  assert.equal(Object.getOwnPropertyDescriptor(decoded, "__proto__").value.polluted, true);
  assert.equal(decoded.polluted, undefined);
});

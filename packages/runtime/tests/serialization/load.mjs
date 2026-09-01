import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const formatUrl = moduleUrl(await compileRuntime("serialization/format.ts"));
const encodeUrl = moduleUrl(await compileRuntime("serialization/encode.ts", { "./format.js": formatUrl }));
const decodeUrl = moduleUrl(await compileRuntime("serialization/decode.ts", { "./format.js": formatUrl }));
const codecUrl = moduleUrl(await compileRuntime("serialization/codec.ts", {
  "./format.js": formatUrl,
  "./encode.js": encodeUrl,
  "./decode.js": decodeUrl,
}));

export const codec = await import(codecUrl);
export const format = await import(formatUrl);
export const profiles = ["queue-v8", "workflow"];

export function encode(value, profile = "workflow") {
  return codec.encodeDurableValue(value, profile);
}
export function decode(bytes, profile = "workflow") {
  return codec.decodeDurableValue(bytes, profile);
}
export function roundTrip(value, profile = "workflow") {
  return decode(encode(value, profile), profile);
}

export function graphEqual(left, right, mapped = new Map()) {
  if (Object.is(left, right)) return true;
  if (typeof left !== typeof right || left === null || right === null || typeof left !== "object") {
    return false;
  }
  if (mapped.has(left)) return mapped.get(left) === right;
  mapped.set(left, right);
  if (Object.getPrototypeOf(left) !== Object.getPrototypeOf(right)) return false;
  if (left instanceof Date) return right instanceof Date && Object.is(left.getTime(), right.getTime());
  if (left instanceof RegExp) {
    return right instanceof RegExp && left.source === right.source && left.flags === right.flags
      && Object.is(left.lastIndex, right.lastIndex);
  }
  if (left instanceof Map) {
    if (!(right instanceof Map) || left.size !== right.size) return false;
    const entries = [...right];
    let index = 0;
    for (const [key, value] of left) {
      const other = entries[index++];
      if (!graphEqual(key, other[0], mapped) || !graphEqual(value, other[1], mapped)) return false;
    }
    return true;
  }
  if (left instanceof Set) {
    if (!(right instanceof Set) || left.size !== right.size) return false;
    const entries = [...right];
    let index = 0;
    for (const value of left) {
      if (!graphEqual(value, entries[index++], mapped)) return false;
    }
    return true;
  }
  if (left instanceof ArrayBuffer) {
    if (!(right instanceof ArrayBuffer) || left.byteLength !== right.byteLength) return false;
    return Buffer.from(left).equals(Buffer.from(right));
  }
  if (ArrayBuffer.isView(left)) {
    return ArrayBuffer.isView(right) && left.constructor === right.constructor
      && left.byteOffset === right.byteOffset && left.byteLength === right.byteLength
      && graphEqual(left.buffer, right.buffer, mapped);
  }
  if (left instanceof Error) {
    if (!(right instanceof Error) || left.name !== right.name || left.message !== right.message) return false;
    const leftCause = Object.prototype.hasOwnProperty.call(left, "cause");
    const rightCause = Object.prototype.hasOwnProperty.call(right, "cause");
    if (leftCause !== rightCause || (leftCause && !graphEqual(left.cause, right.cause, mapped))) return false;
    if ("errors" in left || "errors" in right) {
      if (!graphEqual(left.errors, right.errors, mapped)) return false;
    }
    return true;
  }
  if (Array.isArray(left)) {
    if (!Array.isArray(right) || left.length !== right.length) return false;
    for (let index = 0; index < left.length; index++) {
      if ((index in left) !== (index in right)) return false;
      if (index in left && !graphEqual(left[index], right[index], mapped)) return false;
    }
  }
  const keys = Object.keys(left);
  if (keys.length !== Object.keys(right).length) return false;
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(right, key)) return false;
    if (!graphEqual(left[key], right[key], mapped)) return false;
  }
  return true;
}

export function assertRoundTrip(assert, value, profile = "workflow") {
  const bytes = encode(value, profile);
  const decoded = decode(bytes, profile);
  assert.ok(graphEqual(value, decoded), profile);
  assert.deepEqual(encode(decoded, profile), bytes);
  return decoded;
}

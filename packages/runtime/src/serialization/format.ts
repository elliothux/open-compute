import type { DurableValueLimits, DurableValueProfile } from "./protocol.js";

/** Current schema tag. This is a Day1 format, not a negotiated version. */
export const DURABLE_VALUE_SCHEMA = 1;
/** Magic `OCDV`. */
export const DURABLE_VALUE_MAGIC = new Uint8Array([0x4f, 0x43, 0x44, 0x56]);
export const DURABLE_VALUE_PROFILES = Object.freeze(["queue-v8", "workflow"] as const);
export const DURABLE_VALUE_PROFILE_ID = Object.freeze({ "queue-v8": 1, workflow: 2 });
export const DURABLE_VALUE_LIMITS: { readonly [K in DurableValueProfile]: DurableValueLimits } = {
  "queue-v8": Object.freeze({ maxBytes: 128_000, maxNodes: 32_768, maxDepth: 128 }),
  workflow: Object.freeze({ maxBytes: 1_048_576, maxNodes: 262_144, maxDepth: 128 }),
};
export const TAG = Object.freeze({
  NULL: 0x00, UNDEFINED: 0x01, FALSE: 0x02, TRUE: 0x03, NUMBER: 0x04, BIGINT: 0x05,
  STRING: 0x06, HOLE: 0x07, ARRAY: 0x10, OBJECT: 0x11, NULL_OBJECT: 0x12, DATE: 0x13,
  REGEXP: 0x14, ERROR: 0x15, ARRAY_BUFFER: 0x16, DATA_VIEW: 0x17, TYPED_ARRAY: 0x18,
  MAP: 0x19, SET: 0x1a, REF: 0x1f,
});
export const ERROR_KIND = Object.freeze({
  ERROR: 0, TYPE: 1, RANGE: 2, REFERENCE: 3, SYNTAX: 4, URI: 5, EVAL: 6, AGGREGATE: 7, DOM: 8,
});
export const ERROR_CAUSE = 1;
export const ERROR_ERRORS = 2;
export const ERROR_CODE = 4;

export interface TypedArrayCtor {
  new (buffer: ArrayBuffer, byteOffset?: number, length?: number): ArrayBufferView & ArrayLike<number | bigint>;
  readonly prototype: ArrayBufferView;
  readonly BYTES_PER_ELEMENT: number;
}
export const TYPED_ARRAYS: readonly TypedArrayCtor[] = [
  Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array,
  Float32Array, Float64Array, BigInt64Array, BigUint64Array,
];

const codecFailures = new WeakMap<object, string>();
const rememberFailure = codecFailures.set.bind(codecFailures);
const readFailure = codecFailures.get.bind(codecFailures);
const ObjectCreate = Object.create.bind(Object);
const ObjectDefineProperty = Object.defineProperty.bind(Object);
export const getPrototypeOf = Object.getPrototypeOf.bind(Object);
export const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor.bind(Object);
export const getOwnPropertyNames = Object.getOwnPropertyNames.bind(Object);
export const getOwnPropertySymbols = Object.getOwnPropertySymbols.bind(Object);
export const objectKeys = Object.keys.bind(Object);
export const isArray = Array.isArray.bind(Array);
export const ObjectPrototype: object = Object.prototype;
export const ArrayPrototype: object = Array.prototype;
export const ArrayCtor = Array;
export const DateCtor = Date;
export const RegExpCtor = RegExp;
export const MapCtor = Map;
export const SetCtor = Set;
export const ArrayBufferCtor = ArrayBuffer;
export const DataViewCtor = DataView;
export const ErrorCtor = Error;
export const TypeErrorCtor = TypeError;
export const RangeErrorCtor = RangeError;
export const ReferenceErrorCtor = ReferenceError;
export const SyntaxErrorCtor = SyntaxError;
export const URIErrorCtor = URIError;
export const EvalErrorCtor = EvalError;
export const AggregateErrorCtor = AggregateError;
export const DOMExceptionCtor = typeof DOMException === "function" ? DOMException : undefined;
export const mapForEach = Map.prototype.forEach;
export const setForEach = Set.prototype.forEach;
export const mapSet = Map.prototype.set;
export const setAdd = Set.prototype.add;
export const dateGetTime = Date.prototype.getTime;
export const dateSetTime = Date.prototype.setTime;
export const arrayBufferSlice = ArrayBuffer.prototype.slice;
export const TypedArrayPrototype: object = getPrototypeOf(Uint8Array.prototype);
function requiredGet(source: object, key: string): (this: object) => unknown {
  const getter = getOwnPropertyDescriptor(source, key)?.get;
  if (getter === undefined) throw new Error("DURABLE_VALUE_INTRINSIC_MISSING");
  return getter;
}
export const typedBuffer = requiredGet(TypedArrayPrototype, "buffer") as (this: ArrayBufferView) => ArrayBuffer;
export const typedByteOffset = requiredGet(TypedArrayPrototype, "byteOffset") as (this: ArrayBufferView) => number;
export const typedLength = requiredGet(TypedArrayPrototype, "length") as (this: ArrayBufferView) => number;
export const dataViewBuffer = requiredGet(DataView.prototype, "buffer") as (this: DataView) => ArrayBuffer;
export const dataViewByteOffset = requiredGet(DataView.prototype, "byteOffset") as (this: DataView) => number;
export const dataViewByteLength = requiredGet(DataView.prototype, "byteLength") as (this: DataView) => number;
export const arrayBufferByteLength = requiredGet(ArrayBuffer.prototype, "byteLength") as (this: ArrayBuffer) => number;
const arrayBufferDetached = getOwnPropertyDescriptor(ArrayBuffer.prototype, "detached")?.get as
  ((this: ArrayBuffer) => boolean) | undefined;
const arrayBufferResizable = getOwnPropertyDescriptor(ArrayBuffer.prototype, "resizable")?.get as
  ((this: ArrayBuffer) => boolean) | undefined;
export const regexpSource = requiredGet(RegExp.prototype, "source") as (this: RegExp) => string;
export const regexpFlags = requiredGet(RegExp.prototype, "flags") as (this: RegExp) => string;
export const mapSize = requiredGet(Map.prototype, "size") as (this: Map<unknown, unknown>) => number;
export const setSize = requiredGet(Set.prototype, "size") as (this: Set<unknown>) => number;
const SharedArrayBufferCtor = typeof SharedArrayBuffer === "function" ? SharedArrayBuffer : undefined;
const hostCtors: readonly Function[] = [
  Promise, WeakMap, WeakSet, ReadableStream, WritableStream, TransformStream, Request, Response, Headers,
  ...typeof MessagePort === "function" ? [MessagePort] : [],
  ...typeof Blob === "function" ? [Blob] : [],
  ...typeof URL === "function" ? [URL] : [],
  ...typeof URLSearchParams === "function" ? [URLSearchParams] : [],
  ...typeof WeakRef === "function" ? [WeakRef] : [],
  ...typeof File === "function" ? [File] : [],
];

export function durableValueLimits(profile: DurableValueProfile): DurableValueLimits {
  return DURABLE_VALUE_LIMITS[assertProfile(profile)];
}

export function assertProfile(profile: unknown): DurableValueProfile {
  if (profile === "queue-v8" || profile === "workflow") return profile;
  throw Object.assign(new TypeError("DURABLE_VALUE_PROFILE_UNSUPPORTED"), {
    stableCode: "DURABLE_VALUE_PROFILE_UNSUPPORTED",
  });
}

export function durableValueErrorCode(error: unknown, profile: DurableValueProfile): string | undefined {
  if (error === null || (typeof error !== "object" && typeof error !== "function")) return undefined;
  const code = readFailure(error);
  const expected = codes(assertProfile(profile));
  return code === expected.unsupported || code === expected.tooLarge || code === expected.malformed
    ? code : undefined;
}

export function codes(profile: DurableValueProfile) {
  return profile === "queue-v8"
    ? { unsupported: "QUEUE_V8_UNSUPPORTED", tooLarge: "QUEUE_V8_TOO_LARGE", malformed: "QUEUE_V8_MALFORMED" }
    : {
      unsupported: "WORKFLOW_SERIALIZATION_UNSUPPORTED",
      tooLarge: "WORKFLOW_RESULT_TOO_LARGE",
      malformed: "WORKFLOW_SERIALIZATION_MALFORMED",
    };
}

export function fail(profile: DurableValueProfile, kind: "unsupported" | "tooLarge" | "malformed"): never {
  const code = codes(profile)[kind];
  const error = Object.assign(new (profile === "queue-v8" ? TypeError : Error)(code), { stableCode: code });
  error.stack = `${error.name}: ${code}`;
  rememberFailure(error, code);
  throw error;
}

export function defineData(target: object, key: PropertyKey, value: unknown): void {
  ObjectDefineProperty(target, key, { value, enumerable: true, writable: true, configurable: true });
}

export function createObject(nullPrototype: boolean): object {
  return ObjectCreate(nullPrototype ? null : ObjectPrototype);
}

export function dataDescriptor(value: object, key: PropertyKey) {
  const descriptor = getOwnPropertyDescriptor(value, key);
  if (descriptor === undefined) return undefined;
  if (descriptor.get !== undefined || descriptor.set !== undefined) return "accessor";
  return descriptor;
}

export function enumerableStringKeys(value: object): string[] | undefined {
  if (getOwnPropertySymbols(value).some((symbol) => getOwnPropertyDescriptor(value, symbol)?.enumerable === true)) {
    return undefined;
  }
  return objectKeys(value);
}

export function canonicalIndex(key: string, length: number): number | undefined {
  if (key === "0") return length > 0 ? 0 : undefined;
  if (!/^[1-9][0-9]*$/.test(key)) return undefined;
  const index = Number(key);
  return Number.isSafeInteger(index) && index < length && String(index) === key ? index : undefined;
}

export function typedArrayIndex(value: object): number | undefined {
  const prototype = getPrototypeOf(value);
  const index = TYPED_ARRAYS.findIndex((ctor) => prototype === ctor.prototype);
  return index >= 0 ? index : undefined;
}

export function unsafeBuffer(buffer: ArrayBuffer): boolean {
  if (SharedArrayBufferCtor !== undefined && buffer instanceof SharedArrayBufferCtor) return true;
  if (arrayBufferDetached !== undefined && arrayBufferDetached.call(buffer)) return true;
  if (arrayBufferResizable !== undefined && arrayBufferResizable.call(buffer)) return true;
  try { return arrayBufferByteLength.call(buffer) !== arrayBufferSlice.call(buffer, 0).byteLength; }
  catch { return true; }
}

export function copyBuffer(buffer: ArrayBuffer): ArrayBuffer {
  return arrayBufferSlice.call(buffer, 0);
}

export function isHostObject(value: object): boolean {
  if (SharedArrayBufferCtor !== undefined && value instanceof SharedArrayBufferCtor) return true;
  return hostCtors.some((ctor) => value instanceof (ctor as new () => object));
}

export function errorKindOf(value: object): number | undefined {
  const prototype = getPrototypeOf(value);
  if (prototype === ErrorCtor.prototype) return ERROR_KIND.ERROR;
  if (prototype === TypeErrorCtor.prototype) return ERROR_KIND.TYPE;
  if (prototype === RangeErrorCtor.prototype) return ERROR_KIND.RANGE;
  if (prototype === ReferenceErrorCtor.prototype) return ERROR_KIND.REFERENCE;
  if (prototype === SyntaxErrorCtor.prototype) return ERROR_KIND.SYNTAX;
  if (prototype === URIErrorCtor.prototype) return ERROR_KIND.URI;
  if (prototype === EvalErrorCtor.prototype) return ERROR_KIND.EVAL;
  if (prototype === AggregateErrorCtor.prototype) return ERROR_KIND.AGGREGATE;
  if (DOMExceptionCtor !== undefined && prototype === DOMExceptionCtor.prototype) return ERROR_KIND.DOM;
  return undefined;
}

export function errorConstructor(kind: number): new (message?: string) => Error {
  if (kind === ERROR_KIND.TYPE) return TypeErrorCtor;
  if (kind === ERROR_KIND.RANGE) return RangeErrorCtor;
  if (kind === ERROR_KIND.REFERENCE) return ReferenceErrorCtor;
  if (kind === ERROR_KIND.SYNTAX) return SyntaxErrorCtor;
  if (kind === ERROR_KIND.URI) return URIErrorCtor;
  if (kind === ERROR_KIND.EVAL) return EvalErrorCtor;
  if (kind === ERROR_KIND.ERROR) return ErrorCtor;
  throw new Error("DURABLE_VALUE_INTRINSIC_MISSING");
}

export function wtf8Encode(text: string): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(text.length * 3);
  let offset = 0;
  for (let index = 0; index < text.length; index++) {
    const unit = text.charCodeAt(index);
    if (unit <= 0x7f) {
      out[offset++] = unit;
    } else if (unit <= 0x7ff) {
      out[offset++] = 0xc0 | (unit >> 6);
      out[offset++] = 0x80 | (unit & 0x3f);
    } else if (unit >= 0xd800 && unit <= 0xdbff && index + 1 < text.length) {
      const next = text.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        const code = 0x10000 + ((unit - 0xd800) << 10) + (next - 0xdc00);
        out[offset++] = 0xf0 | (code >> 18);
        out[offset++] = 0x80 | ((code >> 12) & 0x3f);
        out[offset++] = 0x80 | ((code >> 6) & 0x3f);
        out[offset++] = 0x80 | (code & 0x3f);
        index += 1;
      } else {
        out[offset++] = 0xe0 | (unit >> 12);
        out[offset++] = 0x80 | ((unit >> 6) & 0x3f);
        out[offset++] = 0x80 | (unit & 0x3f);
      }
    } else {
      out[offset++] = 0xe0 | (unit >> 12);
      out[offset++] = 0x80 | ((unit >> 6) & 0x3f);
      out[offset++] = 0x80 | (unit & 0x3f);
    }
  }
  return out.subarray(0, offset) as Uint8Array<ArrayBuffer>;
}

export function wtf8Decode(bytes: Uint8Array, malformed: () => never): string {
  const units: number[] = [];
  for (let index = 0; index < bytes.length;) {
    const b0 = bytes[index]!;
    if (b0 <= 0x7f) {
      units.push(b0);
      index += 1;
      continue;
    }
    if (b0 >= 0xc2 && b0 <= 0xdf) {
      const b1 = bytes[index + 1];
      if (b1 === undefined || (b1 & 0xc0) !== 0x80) malformed();
      units.push(((b0 & 0x1f) << 6) | (b1 & 0x3f));
      index += 2;
      continue;
    }
    if (b0 >= 0xe0 && b0 <= 0xef) {
      const b1 = bytes[index + 1], b2 = bytes[index + 2];
      if (b1 === undefined || b2 === undefined || (b1 & 0xc0) !== 0x80 || (b2 & 0xc0) !== 0x80) malformed();
      if (b0 === 0xe0 && b1 < 0xa0) malformed();
      units.push(((b0 & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f));
      index += 3;
      continue;
    }
    if (b0 >= 0xf0 && b0 <= 0xf4) {
      const b1 = bytes[index + 1], b2 = bytes[index + 2], b3 = bytes[index + 3];
      if (b1 === undefined || b2 === undefined || b3 === undefined
          || (b1 & 0xc0) !== 0x80 || (b2 & 0xc0) !== 0x80 || (b3 & 0xc0) !== 0x80) malformed();
      if (b0 === 0xf0 && b1 < 0x90 || b0 === 0xf4 && b1 > 0x8f) malformed();
      const code = ((b0 & 0x07) << 18) | ((b1 & 0x3f) << 12) | ((b2 & 0x3f) << 6) | (b3 & 0x3f);
      if (code < 0x10000 || code > 0x10ffff) malformed();
      units.push(0xd800 + ((code - 0x10000) >> 10), 0xdc00 + ((code - 0x10000) & 0x3ff));
      index += 4;
      continue;
    }
    malformed();
  }
  const parts: string[] = [];
  for (let index = 0; index < units.length; index += 8192) {
    parts.push(String.fromCharCode(...units.slice(index, index + 8192)));
  }
  return parts.join("");
}

export class Writer {
  bytes: Uint8Array<ArrayBuffer>;
  view: DataView;
  offset = 0;
  constructor(readonly maxBytes: number) {
    this.bytes = new Uint8Array(Math.min(256, maxBytes));
    this.view = new DataView(this.bytes.buffer);
  }
  need(count: number, tooLarge: () => never): void {
    const next = this.offset + count;
    if (next > this.maxBytes) tooLarge();
    if (next <= this.bytes.length) return;
    const grown = new Uint8Array(Math.min(this.maxBytes, Math.max(next, this.bytes.length * 2)));
    grown.set(this.bytes.subarray(0, this.offset));
    this.bytes = grown;
    this.view = new DataView(grown.buffer);
  }
  u8(value: number, tooLarge: () => never): void {
    this.need(1, tooLarge);
    this.bytes[this.offset++] = value;
  }
  u32(value: number, tooLarge: () => never): void {
    this.need(4, tooLarge);
    this.view.setUint32(this.offset, value);
    this.offset += 4;
  }
  f64(value: number, tooLarge: () => never): void {
    this.need(8, tooLarge);
    this.view.setFloat64(this.offset, value);
    this.offset += 8;
  }
  bytesOf(value: Uint8Array, tooLarge: () => never): void {
    this.need(value.byteLength, tooLarge);
    this.bytes.set(value, this.offset);
    this.offset += value.byteLength;
  }
  finish(): Uint8Array<ArrayBuffer> {
    return this.bytes.slice(0, this.offset);
  }
}

export class Reader {
  view: DataView;
  offset = 0;
  constructor(readonly bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  need(count: number, malformed: () => never): void {
    if (this.offset + count > this.bytes.byteLength) malformed();
  }
  u8(malformed: () => never): number {
    this.need(1, malformed);
    return this.bytes[this.offset++]!;
  }
  u32(malformed: () => never): number {
    this.need(4, malformed);
    const value = this.view.getUint32(this.offset);
    this.offset += 4;
    return value;
  }
  f64(malformed: () => never): number {
    this.need(8, malformed);
    const value = this.view.getFloat64(this.offset);
    this.offset += 8;
    return value;
  }
  bytesOf(count: number, malformed: () => never): Uint8Array {
    if (count > this.bytes.byteLength - this.offset) malformed();
    const start = this.offset;
    this.offset += count;
    return this.bytes.subarray(start, this.offset);
  }
  end(malformed: () => never): void {
    if (this.offset !== this.bytes.byteLength) malformed();
  }
}

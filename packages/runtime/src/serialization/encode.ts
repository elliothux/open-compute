import type { DurableValueProfile } from "./protocol.js";
import {
  ArrayBufferCtor, ArrayPrototype, DateCtor, DURABLE_VALUE_LIMITS, DURABLE_VALUE_MAGIC,
  DURABLE_VALUE_PROFILE_ID, DURABLE_VALUE_SCHEMA, ERROR_CAUSE, ERROR_CODE, ERROR_ERRORS, ERROR_KIND,
  MapCtor, ObjectPrototype, RegExpCtor, SetCtor, TAG, TYPED_ARRAYS, Writer, arrayBufferByteLength,
  assertProfile, canonicalIndex, copyBuffer, dataDescriptor, dateGetTime, enumerableStringKeys,
  errorKindOf, fail, getOwnPropertyDescriptor, getPrototypeOf, isArray, isHostObject, mapForEach,
  mapSize, regexpFlags,
  regexpSource, setForEach, setSize, typedArrayIndex, typedBuffer, typedByteOffset, typedLength,
  dataViewBuffer, dataViewByteLength, dataViewByteOffset, unsafeBuffer, wtf8Encode,
} from "./format.js";

class Encoder {
  profile: DurableValueProfile;
  maxBytes: number;
  maxNodes: number;
  maxDepth: number;
  writer: Writer;
  seen = new WeakMap<object, number>();
  nodes = 0;
  constructor(profile: DurableValueProfile) {
    this.profile = profile;
    const limits = DURABLE_VALUE_LIMITS[profile];
    this.maxBytes = limits.maxBytes;
    this.maxNodes = limits.maxNodes;
    this.maxDepth = limits.maxDepth;
    this.writer = new Writer(this.maxBytes);
  }
  tooLarge(): never { return fail(this.profile, "tooLarge"); }
  unsupported(): never { return fail(this.profile, "unsupported"); }
  u8(value: number): void { this.writer.u8(value, () => this.tooLarge()); }
  u32(value: number): void { this.writer.u32(value, () => this.tooLarge()); }
  f64(value: number): void { this.writer.f64(value, () => this.tooLarge()); }
  raw(value: Uint8Array): void { this.writer.bytesOf(value, () => this.tooLarge()); }
  string(value: string): void {
    const bytes = wtf8Encode(value);
    this.u32(bytes.byteLength);
    this.raw(bytes);
  }
  begin(value: object): boolean {
    const existing = this.seen.get(value);
    if (existing !== undefined) {
      this.u8(TAG.REF);
      this.u32(existing);
      return false;
    }
    if (this.nodes >= this.maxNodes) this.tooLarge();
    this.seen.set(value, this.nodes);
    this.nodes += 1;
    return true;
  }
  extraOwn(value: object, skip: ReadonlySet<string>, length = 0): void {
    const keys = enumerableStringKeys(value);
    if (keys === undefined) this.unsupported();
    for (const key of keys) {
      if (skip.has(key) || canonicalIndex(key, length) !== undefined) continue;
      const descriptor = dataDescriptor(value, key);
      if (descriptor === undefined) continue;
      this.unsupported();
    }
  }
  value(value: unknown, depth: number): void {
    switch (typeof value) {
      case "undefined": this.u8(TAG.UNDEFINED); return;
      case "boolean": this.u8(value ? TAG.TRUE : TAG.FALSE); return;
      case "number": this.u8(TAG.NUMBER); this.f64(value); return;
      case "bigint": this.encodeBigInt(value); return;
      case "string": this.u8(TAG.STRING); this.string(value); return;
      case "symbol":
      case "function": this.unsupported();
      case "object":
        if (value === null) { this.u8(TAG.NULL); return; }
        this.object(value, depth);
        return;
    }
  }
  encodeBigInt(value: bigint): void {
    this.u8(TAG.BIGINT);
    if (value === 0n) { this.u8(0); this.u32(0); return; }
    this.u8(value < 0n ? 1 : 0);
    let magnitude = value < 0n ? -value : value;
    const bytes: number[] = [];
    while (magnitude > 0n) {
      bytes.push(Number(magnitude & 0xffn));
      magnitude >>= 8n;
    }
    bytes.reverse();
    this.u32(bytes.length);
    this.raw(Uint8Array.from(bytes));
  }
  object(value: object, depth: number): void {
    if (depth >= this.maxDepth) this.tooLarge();
    if (!this.begin(value)) return;
    if (isHostObject(value)) this.unsupported();
    const prototype = getPrototypeOf(value);
    if (isArray(value)) {
      if (prototype !== ArrayPrototype) this.unsupported();
      this.array(value, depth);
      return;
    }
    if (prototype === ObjectPrototype) { this.plain(value, TAG.OBJECT, depth); return; }
    if (prototype === null) { this.plain(value, TAG.NULL_OBJECT, depth); return; }
    if (prototype === DateCtor.prototype) { this.date(value); return; }
    if (prototype === RegExpCtor.prototype) { this.regexp(value); return; }
    if (prototype === MapCtor.prototype) { this.map(value as Map<unknown, unknown>, depth); return; }
    if (prototype === SetCtor.prototype) { this.set(value as Set<unknown>, depth); return; }
    if (prototype === ArrayBufferCtor.prototype) { this.arrayBuffer(value as ArrayBuffer); return; }
    if (prototype === DataView.prototype) { this.dataView(value as DataView, depth); return; }
    const typed = typedArrayIndex(value);
    if (typed !== undefined) { this.typedArray(value as ArrayBufferView, typed, depth); return; }
    const error = errorKindOf(value);
    if (error !== undefined) { this.error(value, error, depth); return; }
    this.unsupported();
  }
  array(value: unknown[], depth: number): void {
    const length = value.length;
    if (typeof length !== "number" || !Number.isSafeInteger(length) || length < 0) this.unsupported();
    if (length > this.maxNodes) this.tooLarge();
    const extras: string[] = [];
    const keys = enumerableStringKeys(value);
    if (keys === undefined) this.unsupported();
    for (const key of keys) {
      if (canonicalIndex(key, length) !== undefined) continue;
      extras.push(key);
    }
    this.u8(TAG.ARRAY);
    this.u32(length);
    this.u32(extras.length);
    for (let index = 0; index < length; index++) {
      const descriptor = dataDescriptor(value, index);
      if (descriptor === undefined) { this.u8(TAG.HOLE); continue; }
      if (descriptor === "accessor") this.unsupported();
      this.value(descriptor.value, depth + 1);
    }
    for (const key of extras) {
      const descriptor = dataDescriptor(value, key);
      if (descriptor === undefined || descriptor === "accessor") this.unsupported();
      this.string(key);
      this.value(descriptor.value, depth + 1);
    }
  }
  plain(value: object, tag: number, depth: number): void {
    const keys = enumerableStringKeys(value);
    if (keys === undefined) this.unsupported();
    this.u8(tag);
    this.u32(keys.length);
    for (const key of keys) {
      const descriptor = dataDescriptor(value, key);
      if (descriptor === undefined || descriptor === "accessor") this.unsupported();
      this.string(key);
      this.value(descriptor.value, depth + 1);
    }
  }
  date(value: object): void {
    this.extraOwn(value, new Set());
    let time: number;
    try { time = dateGetTime.call(value); }
    catch { this.unsupported(); }
    if (typeof time !== "number") this.unsupported();
    this.u8(TAG.DATE);
    this.f64(time);
  }
  regexp(value: object): void {
    this.extraOwn(value, new Set(["lastIndex"]));
    const source = regexpSource.call(value as RegExp);
    const flags = regexpFlags.call(value as RegExp);
    const lastIndex = dataDescriptor(value, "lastIndex");
    if (typeof source !== "string" || typeof flags !== "string"
        || lastIndex === undefined || lastIndex === "accessor" || typeof lastIndex.value !== "number") {
      this.unsupported();
    }
    this.u8(TAG.REGEXP);
    this.string(source);
    this.string(flags);
    this.f64(lastIndex.value);
  }
  map(value: Map<unknown, unknown>, depth: number): void {
    this.extraOwn(value, new Set());
    const size = mapSize.call(value);
    if (typeof size !== "number" || !Number.isSafeInteger(size) || size < 0) this.unsupported();
    this.u8(TAG.MAP);
    this.u32(size);
    let count = 0;
    mapForEach.call(value, (entry, key) => {
      count += 1;
      if (count > size) this.unsupported();
      this.value(key, depth + 1);
      this.value(entry, depth + 1);
    });
    if (count !== size) this.unsupported();
  }
  set(value: Set<unknown>, depth: number): void {
    this.extraOwn(value, new Set());
    const size = setSize.call(value);
    if (typeof size !== "number" || !Number.isSafeInteger(size) || size < 0) this.unsupported();
    this.u8(TAG.SET);
    this.u32(size);
    let count = 0;
    setForEach.call(value, (entry) => {
      count += 1;
      if (count > size) this.unsupported();
      this.value(entry, depth + 1);
    });
    if (count !== size) this.unsupported();
  }
  arrayBuffer(value: ArrayBuffer): void {
    this.extraOwn(value, new Set());
    if (unsafeBuffer(value)) this.unsupported();
    const copy = new Uint8Array(copyBuffer(value));
    this.u8(TAG.ARRAY_BUFFER);
    this.u32(copy.byteLength);
    this.raw(copy);
  }
  dataView(value: DataView, depth: number): void {
    this.extraOwn(value, new Set());
    const buffer = dataViewBuffer.call(value);
    if (!(buffer instanceof ArrayBufferCtor) || unsafeBuffer(buffer)) this.unsupported();
    const byteOffset = dataViewByteOffset.call(value);
    const byteLength = dataViewByteLength.call(value);
    if (!Number.isSafeInteger(byteOffset) || !Number.isSafeInteger(byteLength) || byteOffset < 0 || byteLength < 0) {
      this.unsupported();
    }
    this.u8(TAG.DATA_VIEW);
    this.value(buffer, depth + 1);
    this.u32(byteOffset);
    this.u32(byteLength);
  }
  typedArray(value: ArrayBufferView, type: number, depth: number): void {
    const length = typedLength.call(value);
    this.extraOwn(value, new Set(), length);
    const buffer = typedBuffer.call(value);
    if (!(buffer instanceof ArrayBufferCtor) || unsafeBuffer(buffer)) this.unsupported();
    const byteOffset = typedByteOffset.call(value);
    const bytesPer = TYPED_ARRAYS[type]!.BYTES_PER_ELEMENT;
    if (!Number.isSafeInteger(byteOffset) || !Number.isSafeInteger(length) || byteOffset < 0 || length < 0
        || byteOffset + length * bytesPer > arrayBufferByteLength.call(buffer)) {
      this.unsupported();
    }
    this.u8(TAG.TYPED_ARRAY);
    this.u8(type);
    this.value(buffer, depth + 1);
    this.u32(byteOffset);
    this.u32(length);
  }
  error(value: object, kind: number, depth: number): void {
    const skip = new Set(["name", "message", "cause", "stack"]);
    if (kind === ERROR_KIND.AGGREGATE) skip.add("errors");
    if (kind === ERROR_KIND.DOM) skip.add("code");
    this.extraOwn(value, skip);
    const name = nativeString(value, "name", () => this.unsupported());
    const message = nativeString(value, "message", () => this.unsupported()) ?? "";
    const cause = dataDescriptor(value, "cause");
    const errors = kind === ERROR_KIND.AGGREGATE ? dataDescriptor(value, "errors") : undefined;
    const code = kind === ERROR_KIND.DOM ? dataDescriptor(value, "code") : undefined;
    if (cause === "accessor" || errors === "accessor" || code === "accessor") this.unsupported();
    let flags = 0;
    if (cause !== undefined) flags |= ERROR_CAUSE;
    if (errors !== undefined) flags |= ERROR_ERRORS;
    if (code !== undefined) {
      if (typeof code.value !== "number" || !Number.isSafeInteger(code.value)) this.unsupported();
      flags |= ERROR_CODE;
    }
    this.u8(TAG.ERROR);
    this.u8(kind);
    this.string(name ?? defaultErrorName(kind));
    this.string(message);
    this.u8(flags);
    if (cause !== undefined) this.value(cause.value, depth + 1);
    if (errors !== undefined) this.value(errors.value, depth + 1);
    if (code !== undefined) this.f64(code.value as number);
  }
}

function ownString(value: object, key: string, unsupported: () => never): string | undefined {
  const descriptor = dataDescriptor(value, key);
  if (descriptor === undefined) return undefined;
  if (descriptor === "accessor" || typeof descriptor.value !== "string") unsupported();
  return descriptor.value;
}

function nativeString(value: object, key: string, unsupported: () => never): string | undefined {
  const own = ownString(value, key, unsupported);
  if (own !== undefined) return own;
  const prototype = getPrototypeOf(value);
  if (prototype === null || typeof prototype !== "object") return undefined;
  const descriptor = getOwnPropertyDescriptor(prototype, key);
  if (descriptor === undefined) return undefined;
  if (descriptor.get !== undefined) {
    const result = descriptor.get.call(value);
    return typeof result === "string" ? result : unsupported();
  }
  return typeof descriptor.value === "string" ? descriptor.value : undefined;
}

function defaultErrorName(kind: number): string {
  if (kind === ERROR_KIND.TYPE) return "TypeError";
  if (kind === ERROR_KIND.RANGE) return "RangeError";
  if (kind === ERROR_KIND.REFERENCE) return "ReferenceError";
  if (kind === ERROR_KIND.SYNTAX) return "SyntaxError";
  if (kind === ERROR_KIND.URI) return "URIError";
  if (kind === ERROR_KIND.EVAL) return "EvalError";
  if (kind === ERROR_KIND.AGGREGATE) return "AggregateError";
  if (kind === ERROR_KIND.DOM) return "DOMException";
  return "Error";
}

export function encodeDurableValue(value: unknown, profile: DurableValueProfile): Uint8Array<ArrayBuffer> {
  const encoder = new Encoder(assertProfile(profile));
  encoder.raw(DURABLE_VALUE_MAGIC);
  encoder.u8(DURABLE_VALUE_SCHEMA);
  encoder.u8(DURABLE_VALUE_PROFILE_ID[encoder.profile]);
  encoder.value(value, 0);
  return encoder.writer.finish();
}

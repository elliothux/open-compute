import type { DurableValueProfile } from "./protocol.js";
import {
  ArrayBufferCtor, ArrayCtor, AggregateErrorCtor, DOMExceptionCtor, DURABLE_VALUE_LIMITS,
  DURABLE_VALUE_MAGIC, DURABLE_VALUE_PROFILE_ID, DURABLE_VALUE_SCHEMA, DataViewCtor, DateCtor,
  ERROR_CAUSE, ERROR_CODE, ERROR_ERRORS, ERROR_KIND, MapCtor, Reader, RegExpCtor, SetCtor, TAG,
  TYPED_ARRAYS, assertProfile, createObject, defineData, errorConstructor, fail,
  mapSet, setAdd, wtf8Decode,
} from "./format.js";

class Decoder {
  profile: DurableValueProfile;
  maxNodes: number;
  maxDepth: number;
  reader: Reader;
  nodes: unknown[] = [];
  constructor(bytes: Uint8Array, profile: DurableValueProfile) {
    this.profile = profile;
    const limits = DURABLE_VALUE_LIMITS[profile];
    this.maxNodes = limits.maxNodes;
    this.maxDepth = limits.maxDepth;
    if (bytes.byteLength > limits.maxBytes) fail(profile, "tooLarge");
    this.reader = new Reader(bytes);
  }
  malformed(): never { return fail(this.profile, "malformed"); }
  tooLarge(): never { return fail(this.profile, "tooLarge"); }
  u8(): number { return this.reader.u8(() => this.malformed()); }
  u32(): number { return this.reader.u32(() => this.malformed()); }
  f64(): number { return this.reader.f64(() => this.malformed()); }
  bytes(count: number): Uint8Array { return this.reader.bytesOf(count, () => this.malformed()); }
  string(): string {
    const count = this.u32();
    return wtf8Decode(this.bytes(count), () => this.malformed());
  }
  reserve(): number {
    if (this.nodes.length >= this.maxNodes) this.tooLarge();
    this.nodes.push(undefined);
    return this.nodes.length - 1;
  }
  commit(id: number, value: object): object {
    this.nodes[id] = value;
    return value;
  }
  ref(): unknown {
    const id = this.u32();
    if (id >= this.nodes.length) this.malformed();
    const value = this.nodes[id];
    if (value === undefined) this.malformed();
    return value;
  }
  value(depth: number, allowHole = false): unknown {
    const tag = this.u8();
    switch (tag) {
      case TAG.NULL: return null;
      case TAG.UNDEFINED: return undefined;
      case TAG.FALSE: return false;
      case TAG.TRUE: return true;
      case TAG.NUMBER: return this.f64();
      case TAG.BIGINT: return this.bigint();
      case TAG.STRING: return this.string();
      case TAG.HOLE: return allowHole ? HOLE : this.malformed();
      case TAG.REF: return this.ref();
      default: return this.object(tag, depth);
    }
  }
  bigint(): bigint {
    const sign = this.u8();
    if (sign > 1) this.malformed();
    const count = this.u32();
    const bytes = this.bytes(count);
    if (count === 0) return 0n;
    if (bytes[0] === 0) this.malformed();
    let value = 0n;
    for (const byte of bytes) value = (value << 8n) | BigInt(byte);
    return sign === 1 ? -value : value;
  }
  object(tag: number, depth: number): unknown {
    if (depth >= this.maxDepth) this.tooLarge();
    if (tag === TAG.ARRAY) return this.array(depth);
    if (tag === TAG.OBJECT) return this.plain(false, depth);
    if (tag === TAG.NULL_OBJECT) return this.plain(true, depth);
    if (tag === TAG.DATE) return this.date();
    if (tag === TAG.REGEXP) return this.regexp();
    if (tag === TAG.MAP) return this.map(depth);
    if (tag === TAG.SET) return this.set(depth);
    if (tag === TAG.ARRAY_BUFFER) return this.arrayBuffer();
    if (tag === TAG.DATA_VIEW) return this.dataView(depth);
    if (tag === TAG.TYPED_ARRAY) return this.typedArray(depth);
    if (tag === TAG.ERROR) return this.error(depth);
    return this.malformed();
  }
  array(depth: number): unknown[] {
    const id = this.reserve();
    const length = this.u32();
    const extraCount = this.u32();
    if (length > this.maxNodes) this.tooLarge();
    const value = new ArrayCtor(length) as unknown[];
    this.commit(id, value);
    const seen = new Set<string>();
    for (let index = 0; index < length; index++) {
      const item = this.value(depth + 1, true);
      if (item === HOLE) continue;
      defineData(value, index, item);
      seen.add(String(index));
    }
    for (let extra = 0; extra < extraCount; extra++) {
      const key = this.string();
      if (seen.has(key)) this.malformed();
      seen.add(key);
      defineData(value, key, this.value(depth + 1));
    }
    return value;
  }
  plain(nullPrototype: boolean, depth: number): object {
    const id = this.reserve();
    const count = this.u32();
    const value = createObject(nullPrototype);
    this.commit(id, value);
    const seen = new Set<string>();
    for (let index = 0; index < count; index++) {
      const key = this.string();
      if (seen.has(key)) this.malformed();
      seen.add(key);
      defineData(value, key, this.value(depth + 1));
    }
    return value;
  }
  date(): Date {
    const id = this.reserve();
    const date = new DateCtor(this.f64());
    this.commit(id, date);
    return date;
  }
  regexp(): RegExp {
    const id = this.reserve();
    const source = this.string();
    const flags = this.string();
    const lastIndex = this.f64();
    let value: RegExp;
    try { value = new RegExpCtor(source, flags); }
    catch { this.malformed(); }
    value.lastIndex = lastIndex;
    this.commit(id, value);
    return value;
  }
  map(depth: number): Map<unknown, unknown> {
    const id = this.reserve();
    const size = this.u32();
    const value = new MapCtor();
    this.commit(id, value);
    for (let index = 0; index < size; index++) {
      const key = this.value(depth + 1);
      mapSet.call(value, key, this.value(depth + 1));
    }
    return value;
  }
  set(depth: number): Set<unknown> {
    const id = this.reserve();
    const size = this.u32();
    const value = new SetCtor();
    this.commit(id, value);
    for (let index = 0; index < size; index++) setAdd.call(value, this.value(depth + 1));
    return value;
  }
  arrayBuffer(): ArrayBuffer {
    const id = this.reserve();
    const count = this.u32();
    const copy = new Uint8Array(count);
    copy.set(this.bytes(count));
    this.commit(id, copy.buffer);
    return copy.buffer;
  }
  dataView(depth: number): DataView {
    const id = this.reserve();
    const buffer = this.bufferNode(depth);
    const byteOffset = this.u32();
    const byteLength = this.u32();
    if (byteOffset + byteLength > buffer.byteLength) this.malformed();
    let view: DataView;
    try { view = new DataViewCtor(buffer, byteOffset, byteLength); }
    catch { this.malformed(); }
    this.commit(id, view);
    return view;
  }
  typedArray(depth: number): ArrayBufferView {
    const id = this.reserve();
    const type = this.u8();
    const ctor = TYPED_ARRAYS[type];
    if (ctor === undefined) this.malformed();
    const buffer = this.bufferNode(depth);
    const byteOffset = this.u32();
    const length = this.u32();
    if (byteOffset + length * ctor.BYTES_PER_ELEMENT > buffer.byteLength) this.malformed();
    let view: ArrayBufferView;
    try { view = new ctor(buffer, byteOffset, length); }
    catch { this.malformed(); }
    this.commit(id, view);
    return view;
  }
  bufferNode(depth: number): ArrayBuffer {
    const buffer = this.value(depth + 1);
    if (!(buffer instanceof ArrayBufferCtor)) this.malformed();
    return buffer;
  }
  error(depth: number): Error {
    const id = this.reserve();
    const kind = this.u8();
    if (kind === ERROR_KIND.DOM && DOMExceptionCtor === undefined) this.malformed();
    if (kind > ERROR_KIND.DOM) this.malformed();
    const name = this.string();
    const message = this.string();
    const flags = this.u8();
    let value: Error;
    if (kind === ERROR_KIND.DOM) {
      try { value = new DOMExceptionCtor!(message, name); }
      catch { this.malformed(); }
    } else if (kind === ERROR_KIND.AGGREGATE) {
      value = new AggregateErrorCtor([], message);
    } else {
      value = new (errorConstructor(kind))(message);
    }
    this.commit(id, value);
    if (name !== value.name) defineData(value, "name", name);
    if ((flags & ERROR_CAUSE) !== 0) {
      defineData(value, "cause", this.value(depth + 1));
    }
    if ((flags & ERROR_ERRORS) !== 0) {
      if (kind !== ERROR_KIND.AGGREGATE) this.malformed();
      defineData(value, "errors", this.value(depth + 1));
    }
    if ((flags & ERROR_CODE) !== 0) {
      if (kind !== ERROR_KIND.DOM) this.malformed();
      defineData(value, "code", this.f64());
    }
    if (flags > (ERROR_CAUSE | ERROR_ERRORS | ERROR_CODE)) this.malformed();
    return value;
  }
}

const HOLE = Symbol("durable-hole");

export function decodeDurableValue(bytes: unknown, profile: DurableValueProfile): unknown {
  const expected = assertProfile(profile);
  if (!(bytes instanceof Uint8Array)) fail(expected, "malformed");
  const decoder = new Decoder(bytes, expected);
  for (const byte of DURABLE_VALUE_MAGIC) {
    if (decoder.u8() !== byte) decoder.malformed();
  }
  if (decoder.u8() !== DURABLE_VALUE_SCHEMA) decoder.malformed();
  if (decoder.u8() !== DURABLE_VALUE_PROFILE_ID[expected]) decoder.malformed();
  const value = decoder.value(0);
  decoder.reader.end(() => decoder.malformed());
  return value;
}

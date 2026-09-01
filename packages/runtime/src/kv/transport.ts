import { WorkerEntrypoint } from "cloudflare:workers";
import { bindingError, currentStartupGeneration } from "../loader/host.js";
import type { BindingEnv, ResourceBindingProps } from "../bindings/protocol.js";

const BINDING_TOKEN_HEADER = "x-open-compute-binding-token";
const BINDING_FRAME_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+frame";
const MAX_BINDING_KEY_BYTES = 512;
const MAX_KV_VALUE_BYTES = 25 * 1024 * 1024;
const MAX_KV_METADATA_BYTES = 1024;
const MAX_KV_KEYS = 100;
const MIN_CACHE_TTL_SECONDS = 30;
const MIN_EXPIRATION_TTL_SECONDS = 60;
const LOCAL_CACHE_STATUS = null;

type KvReadType = "text" | "json" | "arrayBuffer" | "stream";
interface KvReadOptions { type: KvReadType; cacheTtl: number | undefined }
interface KvPutOptions {
  expiration?: number | undefined;
  expirationTtl?: number | undefined;
  metadata?: unknown;
  metadataPresent: boolean;
}
interface KvEntry<T> { value: T | null; metadata: unknown; expiration: number | null }

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isUnpairedSurrogate(value: string): boolean {
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return true;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return true;
    }
  }
  return false;
}

function utf8Bytes(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

function assertKey(key: unknown): asserts key is string {
  if (typeof key !== "string" || !key || key === "." || key === ".." || isUnpairedSurrogate(key)) {
    throw new TypeError("KV_KEY_INVALID");
  }
  if (utf8Bytes(key) > MAX_BINDING_KEY_BYTES) throw new TypeError("KV_KEY_TOO_LARGE");
}

function assertPrefix(prefix: unknown): asserts prefix is string {
  if (typeof prefix !== "string" || isUnpairedSurrogate(prefix)) throw new TypeError("KV_KEY_INVALID");
  if (utf8Bytes(prefix) > MAX_BINDING_KEY_BYTES) throw new TypeError("KV_KEY_TOO_LARGE");
}

function assertSafeSeconds(value: unknown, minimum: number): asserts value is number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum) {
    throw new TypeError("KV_INVALID_OPTIONS");
  }
}

function getOptions(input: unknown, many: boolean): KvReadOptions {
  let type: unknown = "text";
  let cacheTtl: number | undefined;
  if (input !== undefined) {
    if (typeof input === "string") type = input;
    else if (record(input)) {
      const keys = Object.keys(input);
      if (keys.some((key) => key !== "type" && key !== "cacheTtl")) {
        throw new TypeError("KV_INVALID_OPTIONS");
      }
      if (input.type !== undefined) type = input.type;
      if (input.cacheTtl !== undefined) {
        assertSafeSeconds(input.cacheTtl, MIN_CACHE_TTL_SECONDS);
        cacheTtl = input.cacheTtl;
      }
    } else throw new TypeError("KV_INVALID_OPTIONS");
  }
  if ((type !== "text" && type !== "json" && type !== "arrayBuffer" && type !== "stream")
      || (many && type !== "text" && type !== "json")) throw new TypeError("KV_INVALID_OPTIONS");
  return { type, cacheTtl };
}

function assertMetadata(value: unknown, seen = new WeakSet<object>()) {
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new TypeError("KV_METADATA_INVALID");
    return;
  }
  if (typeof value !== "object") throw new TypeError("KV_METADATA_INVALID");
  if (seen.has(value)) throw new TypeError("KV_METADATA_INVALID");
  seen.add(value);
  if (Array.isArray(value)) {
    for (const entry of value) assertMetadata(entry, seen);
  } else {
    for (const item of Object.values(value)) assertMetadata(item, seen);
  }
  seen.delete(value);
}

function putOptions(input: unknown): KvPutOptions {
  if (input === undefined) return { metadataPresent: false };
  if (!record(input)) throw new TypeError("KV_INVALID_OPTIONS");
  const keys = Object.keys(input);
  if (keys.some((key) => !["expiration", "expirationTtl", "metadata"].includes(key))) {
    throw new TypeError("KV_INVALID_OPTIONS");
  }
  if (input.expiration !== undefined && input.expirationTtl !== undefined) {
    throw new TypeError("KV_INVALID_OPTIONS");
  }
  if (input.expiration !== undefined) assertSafeSeconds(input.expiration, 1);
  if (input.expirationTtl !== undefined) assertSafeSeconds(input.expirationTtl, MIN_EXPIRATION_TTL_SECONDS);
  const metadataPresent = Object.prototype.hasOwnProperty.call(input, "metadata")
    && input.metadata !== undefined;
  if (metadataPresent) {
    assertMetadata(input.metadata);
    if (utf8Bytes(JSON.stringify(input.metadata)) > MAX_KV_METADATA_BYTES) {
      throw new TypeError("KV_METADATA_TOO_LARGE");
    }
  }
  return {
    expiration: input.expiration,
    expirationTtl: input.expirationTtl,
    metadata: metadataPresent ? input.metadata : undefined,
    metadataPresent,
  };
}

function copyBufferSource(value: ArrayBuffer | ArrayBufferView): Uint8Array {
  try {
    const view = ArrayBuffer.isView(value)
      ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
      : new Uint8Array(value);
    const copy = new Uint8Array(view.byteLength);
    copy.set(view);
    return copy;
  } catch {
    throw new TypeError("KV value must be a string, buffer, view, or ReadableStream");
  }
}

function valueStream(value: unknown): { stream: ReadableStream<unknown>; knownLength: number | undefined } {
  if (typeof value === "string") {
    const bytes = new TextEncoder().encode(value);
    return { stream: new Blob([bytes]).stream(), knownLength: bytes.byteLength };
  }
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
    const bytes = copyBufferSource(value);
    return { stream: new Blob([bytes]).stream(), knownLength: bytes.byteLength };
  }
  if (value instanceof ReadableStream) return { stream: value, knownLength: undefined };
  throw new TypeError("KV value must be a string, buffer, view, or ReadableStream");
}

function framedPutBody(header: KvPutOptions & { key: string }, value: unknown): ReadableStream<Uint8Array> {
  const headerBytes = new TextEncoder().encode(JSON.stringify(header));
  if (headerBytes.byteLength > 4096) throw new TypeError("KV_METADATA_TOO_LARGE");
  const prefix = new Uint8Array(4 + headerBytes.byteLength);
  new DataView(prefix.buffer).setUint32(0, headerBytes.byteLength);
  prefix.set(headerBytes, 4);
  const source = valueStream(value);
  if (source.knownLength !== undefined && source.knownLength > MAX_KV_VALUE_BYTES) {
    throw new TypeError("KV_VALUE_TOO_LARGE");
  }
  const reader = source.stream.getReader();
  let first = true;
  let total = 0;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (first) {
        first = false;
        controller.enqueue(prefix);
        return;
      }
      const next = await reader.read();
      if (next.done) {
        controller.close();
        return;
      }
      let chunk: Uint8Array;
      if (typeof next.value === "string") {
        chunk = new TextEncoder().encode(next.value);
      } else if (next.value instanceof ArrayBuffer || ArrayBuffer.isView(next.value)) {
        chunk = copyBufferSource(next.value);
      } else {
        await reader.cancel();
        controller.error(new TypeError("This ReadableStream did not return bytes."));
        return;
      }
      total += chunk.byteLength;
      if (total > MAX_KV_VALUE_BYTES) {
        const prior = total - chunk.byteLength;
        const firstOverflowByte = chunk.subarray(0, MAX_KV_VALUE_BYTES - prior + 1);
        controller.enqueue(firstOverflowByte);
        await reader.cancel();
        controller.close();
        return;
      }
      controller.enqueue(chunk);
    },
    cancel(reason) { return reader.cancel(reason); },
  });
}

function protocolError(): never {
  throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
}

function viewOf(bytes: Uint8Array): DataView {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
}

function parseJsonBytes(bytes: Uint8Array): unknown {
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    protocolError();
  }
}

function decodeExpiration(expiration: bigint): number | null {
  if (expiration === -1n) return null;
  if (expiration < 0n || expiration > BigInt(Number.MAX_SAFE_INTEGER)) protocolError();
  return Number(expiration);
}

function assertFoundMarker(found: number): asserts found is 0 | 1 {
  if (found !== 0 && found !== 1) protocolError();
}

function assertMissingHeader(expiration: bigint, metadataLength: number) {
  if (expiration !== -1n || metadataLength !== 0xffffffff) protocolError();
}

function assertMetadataLength(metadataLength: number) {
  if (metadataLength !== 0xffffffff && metadataLength > MAX_KV_METADATA_BYTES) protocolError();
}

function assertValueLength(valueLength: number) {
  if (valueLength === 0xffffffff || valueLength > MAX_KV_VALUE_BYTES) protocolError();
}

function decodeValue(bytes: Uint8Array | null, type: KvReadType): unknown {
  if (bytes === null) return null;
  if (type === "arrayBuffer") return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const text = new TextDecoder().decode(bytes);
  if (type === "json") return JSON.parse(text);
  return text;
}

async function decodeStreamValue(stream: ReadableStream<Uint8Array> | null, type: KvReadType): Promise<unknown> {
  if (stream === null || type === "stream") return stream;
  try {
    const response = new Response(stream);
    if (type === "arrayBuffer") return await response.arrayBuffer();
    const text = await response.text();
    if (type === "json") return JSON.parse(text);
    return text;
  } catch (error) {
    try { await stream.cancel(); } catch { /* best effort */ }
    throw error;
  }
}

function metadataResult(value: unknown, metadata: unknown) {
  return { value, metadata, cacheStatus: LOCAL_CACHE_STATUS };
}

function openFrameReader(response: Response) {
  if (!response.body) protocolError();
  const reader = response.body.getReader();
  let buffered: Uint8Array = new Uint8Array(0);
  let offset = 0;
  let settled = false;
  const finish = async (cancelStream: boolean, reason?: unknown) => {
    if (settled) return;
    settled = true;
    if (cancelStream) {
      try { await reader.cancel(reason); } catch { /* already closed or cancelled */ }
    }
    try { reader.releaseLock(); } catch { /* lock already released */ }
  };
  const exact = async (length: number) => {
    if (!Number.isInteger(length) || length < 0 || length > MAX_KV_VALUE_BYTES) protocolError();
    const output = new Uint8Array(length);
    let written = 0;
    while (written < length) {
      if (offset === buffered.byteLength) {
        const next = await reader.read();
        if (next.done || !(next.value instanceof Uint8Array)) protocolError();
        buffered = next.value;
        offset = 0;
      }
      const count = Math.min(length - written, buffered.byteLength - offset);
      output.set(buffered.subarray(offset, offset + count), written);
      offset += count;
      written += count;
    }
    return output;
  };
  const leftover = () => buffered.byteLength - offset;
  const assertConsumed = async () => {
    if (leftover() !== 0) protocolError();
    const terminal = await reader.read();
    if (!terminal.done) protocolError();
    await finish(false);
  };
  return {
    reader,
    exact,
    leftover,
    cancel: (reason?: unknown) => finish(true, reason),
    complete: () => finish(false),
    assertConsumed,
    state: () => ({ buffered, offset }),
    advance(count: number) { offset += count; },
  };
}

async function readMetadata(exact: (length: number) => Promise<Uint8Array>, metadataLength: number): Promise<unknown> {
  assertMetadataLength(metadataLength);
  if (metadataLength === 0xffffffff) return null;
  return parseJsonBytes(await exact(metadataLength));
}

async function decodeSingleEntry(response: Response): Promise<KvEntry<ReadableStream<Uint8Array>>> {
  const frame = openFrameReader(response);
  let handedOff = false;
  try {
    const prefix = await frame.exact(17);
    if (new TextDecoder().decode(prefix.subarray(0, 4)) !== "KVS1") protocolError();
    const view = viewOf(prefix);
    const found = view.getUint8(4);
    const expiration = view.getBigInt64(5);
    const metadataLength = view.getUint32(13);
    assertFoundMarker(found);
    if (found === 0) {
      assertMissingHeader(expiration, metadataLength);
      if (viewOf(await frame.exact(4)).getUint32(0) !== 0xffffffff) protocolError();
      await frame.assertConsumed();
      return { value: null, metadata: null, expiration: null };
    }
    const metadata = await readMetadata(frame.exact, metadataLength);
    const valueLength = viewOf(await frame.exact(4)).getUint32(0);
    assertValueLength(valueLength);
    let remaining = valueLength;
    const { reader } = frame;
    const value = new ReadableStream<Uint8Array>({
      async pull(controller) {
        try {
          let { buffered, offset } = frame.state();
          if (remaining === 0) {
            if (offset < buffered.byteLength) protocolError();
            const terminal = await reader.read();
            if (!terminal.done) protocolError();
            await frame.complete();
            controller.close();
            return;
          }
          if (offset < buffered.byteLength) {
            const count = Math.min(remaining, buffered.byteLength - offset);
            controller.enqueue(buffered.subarray(offset, offset + count));
            frame.advance(count);
            remaining -= count;
            return;
          }
          const next = await reader.read();
          if (next.done || !(next.value instanceof Uint8Array) || next.value.byteLength > remaining) {
            protocolError();
          }
          remaining -= next.value.byteLength;
          controller.enqueue(next.value);
        } catch (error) {
          await frame.cancel();
          controller.error(error instanceof Error ? error : bindingError("KV_INTERNAL_PROTOCOL_ERROR"));
        }
      },
      cancel(reason) {
        remaining = 0;
        return frame.cancel(reason);
      },
    });
    handedOff = true;
    return { value, metadata, expiration: decodeExpiration(expiration) };
  } finally {
    if (!handedOff) await frame.cancel();
  }
}

async function decodeBulkEntries(response: Response, expected: number): Promise<KvEntry<Uint8Array>[]> {
  const frame = openFrameReader(response);
  try {
    const magic = await frame.exact(4);
    const countBytes = await frame.exact(2);
    if (new TextDecoder().decode(magic) !== "KVB1") protocolError();
    const count = viewOf(countBytes).getUint16(0);
    if (count !== expected) protocolError();
    const entries: KvEntry<Uint8Array>[] = [];
    for (let i = 0; i < count; i++) {
      const head = await frame.exact(13);
      const view = viewOf(head);
      const found = view.getUint8(0);
      const expiration = view.getBigInt64(1);
      const metadataLength = view.getUint32(9);
      assertFoundMarker(found);
      if (found === 0) {
        assertMissingHeader(expiration, metadataLength);
        if (viewOf(await frame.exact(4)).getUint32(0) !== 0xffffffff) protocolError();
        entries.push({ value: null, metadata: null, expiration: null });
        continue;
      }
      const metadata = await readMetadata(frame.exact, metadataLength);
      const valueLength = viewOf(await frame.exact(4)).getUint32(0);
      assertValueLength(valueLength);
      entries.push({
        value: await frame.exact(valueLength),
        metadata,
        expiration: decodeExpiration(expiration),
      });
    }
    await frame.assertConsumed();
    return entries;
  } finally {
    await frame.cancel();
  }
}

function normalizeListPrefix(value: unknown): string {
  if (value === undefined || value === null) return "";
  assertPrefix(value);
  return value;
}

function normalizeListCursor(value: unknown): string | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string") throw new TypeError("KV_INVALID_OPTIONS");
  return value;
}

export class KVNamespace extends WorkerEntrypoint<BindingEnv, ResourceBindingProps> {
  #props() {
    const props = this.ctx.props;
    if (!props
      || typeof props.bindingId !== "string"
      || typeof props.deploymentId !== "string"
      || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
      || !Number.isSafeInteger(props.resourceSpecGeneration)
      || props.resourceSpecGeneration < 1) {
      throw bindingError("BINDING_PROTOCOL_ERROR");
    }
    return props;
  }

  async #request(operation: string, body: BodyInit, permission: "read" | "write") {
    const props = this.#props();
    if (!props.permissions[permission]) {
      throw bindingError("BINDING_PERMISSION_DENIED");
    }
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/kv/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": BINDING_FRAME_CONTENT_TYPE,
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-deployment-id": props.deploymentId,
          "x-open-compute-descriptor-sha256": props.descriptorSha256,
          "x-open-compute-request-id": crypto.randomUUID(),
        },
        body,
      },
    );
    if (!response.ok) {
      const code = response.headers.get("x-open-compute-error-code") || "BINDING_PROTOCOL_ERROR";
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError(code);
    }
    return response;
  }

  #entries(operation: "get" | "get-with-metadata", keys: readonly string[], options: KvReadOptions): Promise<[KvEntry<ReadableStream<Uint8Array>>]>;
  #entries(operation: "get-many", keys: readonly string[], options: KvReadOptions): Promise<KvEntry<Uint8Array>[]>;
  async #entries(operation: "get" | "get-with-metadata" | "get-many", keys: readonly string[], options: KvReadOptions): Promise<[KvEntry<ReadableStream<Uint8Array>>] | KvEntry<Uint8Array>[]> {
    const response = await this.#request(
      operation,
      JSON.stringify({ keys, cacheTtl: options.cacheTtl }),
      "read",
    );
    if (operation === "get" || operation === "get-with-metadata") {
      return [await decodeSingleEntry(response)];
    }
    return decodeBulkEntries(response, keys.length);
  }

  async get(keyOrKeys: string | string[], typeOrOptions?: unknown): Promise<unknown> {
    const many = Array.isArray(keyOrKeys);
    const keys = many ? keyOrKeys : [keyOrKeys];
    if (many && (keys.length === 0 || keys.length > MAX_KV_KEYS)) throw new TypeError("KV_TOO_MANY_KEYS");
    for (const key of keys) assertKey(key);
    const options = getOptions(typeOrOptions, many);
    if (!many) {
      const [entry] = await this.#entries("get", keys, options);
      return decodeStreamValue(entry.value, options.type);
    }
    const entries = await this.#entries("get-many", keys, options);
    const result = new Map<string, unknown>();
    for (let i = 0; i < keys.length; i++) {
      if (!result.has(keys[i]!)) result.set(keys[i]!, decodeValue(entries[i]!.value, options.type));
    }
    return result;
  }

  async getWithMetadata(keyOrKeys: string | string[], typeOrOptions?: unknown) {
    const many = Array.isArray(keyOrKeys);
    const keys = many ? keyOrKeys : [keyOrKeys];
    if (many && (keys.length === 0 || keys.length > MAX_KV_KEYS)) throw new TypeError("KV_TOO_MANY_KEYS");
    for (const key of keys) assertKey(key);
    const options = getOptions(typeOrOptions, many);
    if (!many) {
      const [entry] = await this.#entries("get-with-metadata", keys, options);
      return metadataResult(await decodeStreamValue(entry.value, options.type), entry.metadata);
    }
    const entries = await this.#entries("get-many", keys, options);
    const result = new Map<string, Omit<ReturnType<typeof metadataResult>, "cacheStatus"> | null>();
    for (let i = 0; i < keys.length; i++) {
      if (!result.has(keys[i]!)) {
        const entry = entries[i]!;
        result.set(keys[i]!, entry.value === null && entry.metadata === null
          ? null
          : { value: decodeValue(entry.value, options.type), metadata: entry.metadata });
      }
    }
    return result;
  }

  async put(key: string, value: unknown, options?: unknown) {
    assertKey(key);
    const normalized = putOptions(options);
    const header = {
      key,
      expiration: normalized.expiration,
      expirationTtl: normalized.expirationTtl,
      metadata: normalized.metadata,
      metadataPresent: normalized.metadataPresent,
    };
    await this.#request("put", framedPutBody(header, value), "write");
  }

  async delete(key: string) {
    assertKey(key);
    await this.#request("delete", JSON.stringify({ key }), "write");
  }

  async list(options: unknown = {}) {
    if (!record(options)) throw new TypeError("KV_INVALID_OPTIONS");
    if (Object.keys(options).some((key) => !["prefix", "limit", "cursor"].includes(key))) {
      throw new TypeError("KV_INVALID_OPTIONS");
    }
    const prefix = normalizeListPrefix(options.prefix);
    const limit = options.limit === undefined ? 1000 : options.limit;
    if (typeof limit !== "number" || !Number.isSafeInteger(limit) || limit < 1 || limit > 1000) {
      throw new TypeError("KV_INVALID_OPTIONS");
    }
    const cursor = normalizeListCursor(options.cursor);
    const response = await this.#request(
      "list",
      JSON.stringify({ prefix, limit, cursor: cursor ?? null }),
      "read",
    );
    const result: unknown = await response.json();
    if (!record(result) || !Array.isArray(result.keys) || typeof result.list_complete !== "boolean"
        || (result.cursor !== null && typeof result.cursor !== "string")) {
      throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    }
    const keys: { name: string; expiration?: number; metadata?: unknown }[] = [];
    for (const key of result.keys as unknown[]) {
      if (!record(key) || typeof key.name !== "string"
          || (key.expiration !== null && (typeof key.expiration !== "number" || !Number.isSafeInteger(key.expiration)))) {
        throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
      }
      keys.push({
        name: key.name,
        ...(key.expiration === null ? {} : { expiration: key.expiration }),
        ...(key.metadata === null ? {} : { metadata: key.metadata }),
      });
    }
    if (result.list_complete) {
      return { keys, list_complete: true as const, cacheStatus: LOCAL_CACHE_STATUS };
    }
    if (typeof result.cursor !== "string") throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    return { keys, list_complete: false as const, cursor: result.cursor, cacheStatus: LOCAL_CACHE_STATUS };
  }

  async fetch(): Promise<never> {
    throw bindingError("BINDING_PERMISSION_DENIED");
  }
}

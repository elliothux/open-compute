interface KvRawTransport {
  get(key: unknown, options?: unknown): Promise<unknown>;
  getWithMetadata(key: unknown, options?: unknown): Promise<unknown>;
  put(key: unknown, value: unknown, options?: unknown): Promise<void>;
  delete(key: unknown): Promise<void>;
  list(options?: unknown): Promise<unknown>;
}

const MAX_KEY_BYTES = 512;
const MAX_VALUE_BYTES = 25 * 1024 * 1024;
const MAX_METADATA_BYTES = 1024;
const MAX_BULK_KEYS = 100;
const MAX_LIST_LIMIT = 1000;
const MIN_CACHE_TTL = 30;
const MIN_EXPIRATION_TTL = 60;
const encoder = new TextEncoder();

const transports = new WeakMap<object, KvRawTransport>();

function transport(owner: object): KvRawTransport {
  const raw = transports.get(owner);
  if (!raw) throw new TypeError("KV_INTERNAL_PROTOCOL_ERROR");
  return raw;
}

function snapshotBufferSource(value: unknown): unknown {
  if (!(value instanceof ArrayBuffer) && !ArrayBuffer.isView(value)) return value;
  try {
    const view = ArrayBuffer.isView(value)
      ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
      : new Uint8Array(value);
    const snapshot = new Uint8Array(view.byteLength);
    snapshot.set(view);
    return snapshot;
  } catch {
    throw invalidPutValue();
  }
}

function invalidPutValue(): TypeError {
  return new TypeError(
    "KV put() accepts only strings, ArrayBuffers, ArrayBufferViews, and ReadableStreams as values.",
  );
}

function domString(value: unknown): string {
  if (typeof value === "symbol") throw new TypeError("Cannot convert a Symbol value to a string");
  return `${value}`;
}

function hasUnpairedSurrogate(value: string): boolean {
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return true;
      index++;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return true;
    }
  }
  return false;
}

function containerString(value: string): string {
  const normalized: string[] = [];
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        normalized.push(value[index]!, value[++index]!);
      } else {
        normalized.push("\ufffd\ufffd\ufffd");
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      normalized.push("\ufffd\ufffd\ufffd");
    } else {
      normalized.push(value[index]!);
    }
  }
  return normalized.join("");
}

function integer(value: unknown): number {
  const number = Number(value);
  if (!Number.isFinite(number) || number === 0) return 0;
  return Math.trunc(number);
}

function normalizeKey(value: unknown, method: "GET" | "PUT" | "DELETE"): string {
  const name = domString(value);
  if (name === "") throw new TypeError("Key name cannot be empty.");
  if (name === ".") throw new TypeError('"." is not allowed as a key name.');
  if (name === "..") throw new TypeError('".." is not allowed as a key name.');
  if (hasUnpairedSurrogate(name)) throw new Error(`KV ${method} failed: 400 Could not URL-decode key name`);
  const length = encoder.encode(name).byteLength;
  if (length > MAX_KEY_BYTES) {
    throw new Error(
      `KV ${method} failed: 414 UTF-8 encoded length of ${length} exceeds key length limit of ${MAX_KEY_BYTES}.`,
    );
  }
  return name;
}

function normalizeBulkKeys(values: unknown[]): string[] {
  const names = values.map(item => containerString(domString(item)));
  if (names.length === 0) {
    throw new Error("KV GET_BULK failed: 400 You must request a minimum of 1 key");
  }
  if (names.length > MAX_BULK_KEYS) {
    throw new Error(`KV GET_BULK failed: 400 You can request a maximum of ${MAX_BULK_KEYS} keys`);
  }
  for (const name of names) {
    if (name === "" || name === "." || name === "..") {
      throw new Error(`KV GET_BULK failed: 400 Key name ${name} is not legal`);
    }
    const length = encoder.encode(name).byteLength;
    if (length > MAX_KEY_BYTES) {
      throw new Error(`KV GET_BULK failed: 414 Encoded length of ${length} is too long`);
    }
  }
  return names;
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && (typeof value === "object" || typeof value === "function")
    ? value as Record<string, unknown>
    : undefined;
}

function getOptions(input: unknown, bulk: boolean): { type: string; cacheTtl?: number } {
  let type = "text";
  let cacheTtl: number | undefined;
  if (input !== undefined && input !== null) {
    const options = object(input);
    if (options === undefined) {
      type = domString(input);
    } else {
      if (options.type !== undefined) type = domString(options.type);
      if (options.cacheTtl !== undefined) cacheTtl = integer(options.cacheTtl);
    }
  }
  if (bulk) {
    if (type !== "text" && type !== "json") {
      throw new Error(`KV GET_BULK failed: 400 "${type}" is not a valid type. Use "json" or "text"`);
    }
  } else if (!["text", "json", "arrayBuffer", "stream"].includes(type)) {
    throw new TypeError(
      'Unknown response type. Possible types are "text", "arrayBuffer", "json", and "stream".',
    );
  }
  if (cacheTtl !== undefined && cacheTtl < MIN_CACHE_TTL) {
    const method = bulk ? "GET_BULK" : "GET";
    throw new Error(
      `KV ${method} failed: 400 Invalid cache_ttl of ${cacheTtl}. Cache TTL must be at least ${MIN_CACHE_TTL}.`,
    );
  }
  return { type, ...(cacheTtl === undefined ? {} : { cacheTtl }) };
}

function putOptions(input: unknown): Record<string, unknown> | undefined {
  const options = object(input);
  if (options === undefined) return undefined;
  const normalized: Record<string, unknown> = {};
  if (options.expirationTtl !== undefined) {
    const expirationTtl = integer(options.expirationTtl);
    if (expirationTtl < MIN_EXPIRATION_TTL) {
      throw new Error(
        `KV PUT failed: 400 Invalid expiration_ttl of ${expirationTtl}. Expiration TTL must be at least ${MIN_EXPIRATION_TTL}.`,
      );
    }
    normalized.expirationTtl = expirationTtl;
  } else if (options.expiration !== undefined) {
    const expiration = integer(options.expiration);
    if (expiration < Math.floor(Date.now() / 1000) + MIN_EXPIRATION_TTL) {
      throw new Error(
        `KV PUT failed: 400 Invalid expiration of ${expiration}. Please specify integer greater than the current number of seconds since the UNIX epoch.`,
      );
    }
    normalized.expiration = expiration;
  }
  if (options.metadata !== undefined && options.metadata !== null) {
    const json = JSON.stringify(options.metadata);
    if (json !== undefined) {
      if (encoder.encode(json).byteLength > MAX_METADATA_BYTES) {
        throw new Error("KV PUT failed: 413 Payload Too Large");
      }
      normalized.metadata = JSON.parse(json) as unknown;
    }
  }
  return Object.keys(normalized).length === 0 ? undefined : normalized;
}

function putValue(value: unknown): unknown {
  const snapshot = snapshotBufferSource(value);
  if (typeof snapshot !== "string" && !(snapshot instanceof Uint8Array)
      && !(snapshot instanceof ReadableStream)) throw invalidPutValue();
  const length = typeof snapshot === "string" ? encoder.encode(snapshot).byteLength
    : snapshot instanceof Uint8Array ? snapshot.byteLength : undefined;
  if (length !== undefined && length > MAX_VALUE_BYTES) {
    throw new Error(`KV PUT failed: 413 Value length of ${length} exceeds limit of ${MAX_VALUE_BYTES}.`);
  }
  return snapshot;
}

function listOptions(input: unknown): Record<string, unknown> | undefined {
  const options = object(input);
  if (options === undefined) return undefined;
  const normalized: Record<string, unknown> = {};
  if (options.prefix !== undefined && options.prefix !== null) {
    const prefix = containerString(domString(options.prefix));
    const length = encoder.encode(prefix).byteLength;
    if (length > MAX_KEY_BYTES) {
      throw new Error(
        `KV GET failed: 414 UTF-8 encoded length of ${length} exceeds key length limit of ${MAX_KEY_BYTES}.`,
      );
    }
    normalized.prefix = prefix;
  }
  if (options.cursor !== undefined && options.cursor !== null) normalized.cursor = domString(options.cursor);
  if (options.limit !== undefined) {
    const limit = integer(options.limit);
    if (limit > MAX_LIST_LIMIT) {
      throw new Error(
        `KV GET failed: 400 Invalid key_count_limit of ${limit}. Please specify integer less than ${MAX_LIST_LIMIT}.`,
      );
    }
    if (limit > 0) normalized.limit = limit;
  }
  return Object.keys(normalized).length === 0 ? undefined : normalized;
}

function privateErrorCode(error: unknown): string | undefined {
  if (error === null || typeof error !== "object") return undefined;
  for (const key of ["stableCode", "message"]) {
    let descriptor: PropertyDescriptor | undefined;
    try {
      descriptor = Object.getOwnPropertyDescriptor(error, key);
    } catch {
      return undefined;
    }
    if (typeof descriptor?.value === "string") return descriptor.value;
  }
  return undefined;
}

async function publicCall<T>(call: () => Promise<T>, operation: "GET" | "PUT"): Promise<T> {
  try {
    return await call();
  } catch (error) {
    const code = privateErrorCode(error);
    if (code === "KV_CURSOR_INVALID") {
      throw new Error("KV GET failed: 400 Invalid cursor");
    }
    if (operation === "PUT" && code === "KV_VALUE_TOO_LARGE") {
      throw new Error(`KV PUT failed: 413 Value length of ${MAX_VALUE_BYTES + 1} exceeds limit of ${MAX_VALUE_BYTES}.`);
    }
    throw error;
  }
}

/** Tenant-local KV facade implements the pinned Cloudflare conversion and error boundary. */
export class KVNamespace {
  constructor(raw: KvRawTransport) {
    transports.set(this, raw);
    Object.freeze(this);
  }

  async get(keyOrKeys: unknown, options?: unknown) {
    const bulk = Array.isArray(keyOrKeys);
    const normalizedKey = bulk
      ? normalizeBulkKeys(keyOrKeys)
      : normalizeKey(keyOrKeys, "GET");
    const normalized = getOptions(options, bulk);
    return publicCall(
      () => transport(this).get(normalizedKey, normalized),
      "GET",
    );
  }

  async getWithMetadata(keyOrKeys: unknown, options?: unknown) {
    const bulk = Array.isArray(keyOrKeys);
    const normalizedKey = bulk
      ? normalizeBulkKeys(keyOrKeys)
      : normalizeKey(keyOrKeys, "GET");
    const normalized = getOptions(options, bulk);
    return publicCall(
      () => transport(this).getWithMetadata(normalizedKey, normalized),
      "GET",
    );
  }

  async put(keyValue: unknown, value: unknown, options?: unknown) {
    return publicCall(
      () => transport(this).put(normalizeKey(keyValue, "PUT"), putValue(value), putOptions(options)),
      "PUT",
    );
  }

  async delete(value: unknown) {
    return transport(this).delete(normalizeKey(value, "DELETE"));
  }

  async list(options?: unknown) {
    return publicCall(() => transport(this).list(listOptions(options)), "GET");
  }
}

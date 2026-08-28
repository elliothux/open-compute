import type { R2Condition, R2HttpMetadata, R2Metadata, R2Range, R2RawTransport } from "./protocol.js";

const bucketState = new WeakMap<object, R2RawTransport>();
const bodyState = new WeakMap<object, { response: Response; claimed: boolean }>();
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function typeError(code: string): never {
  throw new TypeError(code);
}

function assertObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    typeError("R2_INVALID_OPTIONS");
  }
  return value as Record<string, unknown>;
}

function assertKeys(input: Record<string, unknown>, allowed: readonly string[]) {
  if (Object.keys(input).some((key) => !allowed.includes(key))) {
    typeError("R2_INVALID_OPTIONS");
  }
}

function assertKey(value: unknown): string {
  if (typeof value !== "string") typeError("R2_KEY_INVALID");
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) typeError("R2_KEY_INVALID");
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      typeError("R2_KEY_INVALID");
    }
  }
  return value;
}

function safeInteger(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) typeError("R2_INVALID_OPTIONS");
  return value;
}

function etags(value: unknown): string[] {
  if (value == null) return [];
  return (Array.isArray(value) ? value : [value]).map((item) => String(item));
}

function dateMillis(value: unknown): number {
  if (!(value instanceof Date) && typeof value !== "string" && typeof value !== "number") typeError("R2_INVALID_OPTIONS");
  const date = value instanceof Date ? value : new Date(value);
  const millis = date.getTime();
  if (!Number.isFinite(millis)) typeError("R2_INVALID_OPTIONS");
  return millis;
}

function normalizeCondition(value: unknown): R2Condition | undefined {
  if (value == null) return undefined;
  if (value instanceof Headers) {
    const matches = value.get("if-match");
    const differs = value.get("if-none-match");
    const before = value.get("if-unmodified-since");
    const after = value.get("if-modified-since");
    return {
      etagMatches: matches ? matches.split(",").map((item) => item.trim()) : [],
      etagDoesNotMatch: differs ? differs.split(",").map((item) => item.trim()) : [],
      uploadedBefore: before ? dateMillis(before) : undefined,
      uploadedAfter: after ? dateMillis(after) : undefined,
    };
  }
  const input = assertObject(value);
  assertKeys(input, ["etagMatches", "etagDoesNotMatch", "uploadedBefore", "uploadedAfter"]);
  return {
    etagMatches: etags(input.etagMatches),
    etagDoesNotMatch: etags(input.etagDoesNotMatch),
    uploadedBefore: input.uploadedBefore == null ? undefined : dateMillis(input.uploadedBefore),
    uploadedAfter: input.uploadedAfter == null ? undefined : dateMillis(input.uploadedAfter),
  };
}

function normalizeRange(value: unknown): R2Range | undefined {
  if (value == null) return undefined;
  if (value instanceof Headers) {
    const header = value.get("range");
    if (!header) return undefined;
    if (header.includes(",")) typeError("R2_RANGE_NOT_SUPPORTED");
    const match = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
    if (!match || (!match[1] && !match[2])) typeError("R2_INVALID_OPTIONS");
    if (!match[1]) return { suffix: safeInteger(Number(match[2])) };
    const offset = safeInteger(Number(match[1]));
    if (!match[2]) return { offset };
    const end = safeInteger(Number(match[2]));
    if (end < offset) typeError("R2_INVALID_OPTIONS");
    return { offset, length: end - offset + 1 };
  }
  const input = assertObject(value);
  const allowed = new Set(["offset", "length", "suffix"]);
  if (Object.keys(input).some((key) => !allowed.has(key))) typeError("R2_INVALID_OPTIONS");
  const out: R2Range = {};
  if (input.offset != null) out.offset = safeInteger(input.offset);
  if (input.length != null) out.length = safeInteger(input.length);
  if (input.suffix != null) out.suffix = safeInteger(input.suffix);
  if (out.suffix != null && (out.offset != null || out.length != null)) {
    typeError("R2_INVALID_OPTIONS");
  }
  if (out.offset == null && out.length == null && out.suffix == null) {
    typeError("R2_INVALID_OPTIONS");
  }
  return out;
}

function normalizeHttpMetadata(value: unknown): R2HttpMetadata {
  if (value == null) return {};
  if (value instanceof Headers) {
    const expires = value.get("expires");
    return {
      contentType: value.get("content-type") || undefined,
      contentLanguage: value.get("content-language") || undefined,
      contentDisposition: value.get("content-disposition") || undefined,
      contentEncoding: value.get("content-encoding") || undefined,
      cacheControl: value.get("cache-control") || undefined,
      cacheExpiry: expires ? dateMillis(expires) : undefined,
    };
  }
  const input = assertObject(value);
  assertKeys(input, [
    "contentType",
    "contentLanguage",
    "contentDisposition",
    "contentEncoding",
    "cacheControl",
    "cacheExpiry",
  ]);
  const out: R2HttpMetadata = {};
  for (const name of [
    "contentType",
    "contentLanguage",
    "contentDisposition",
    "contentEncoding",
    "cacheControl",
  ] as const) {
    if (input[name] != null) out[name] = String(input[name]);
  }
  if (input.cacheExpiry != null) out.cacheExpiry = dateMillis(input.cacheExpiry);
  return out;
}

function normalizeCustomMetadata(value: unknown): Record<string, string> {
  if (value == null) return {};
  const input = assertObject(value);
  const out: Record<string, string> = {};
  for (const [key, item] of Object.entries(input)) out[key] = String(item);
  return out;
}

function normalizeMd5(value: unknown): string | number[] | undefined {
  if (value == null || typeof value === "string") return value ?? undefined;
  if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
  if (ArrayBuffer.isView(value)) {
    return Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
  }
  typeError("R2_INVALID_OPTIONS");
}

function oneChunk(bytes: Uint8Array): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    },
  });
}

function putBody(value: unknown): ReadableStream<unknown> {
  if (value == null) return oneChunk(new Uint8Array());
  if (typeof value === "string") return oneChunk(encoder.encode(value));
  if (value instanceof ArrayBuffer) return oneChunk(new Uint8Array(value.slice(0)));
  if (ArrayBuffer.isView(value)) {
    const copy = new Uint8Array(value.byteLength);
    copy.set(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
    return oneChunk(copy);
  }
  if (value instanceof Blob) return value.stream();
  if (value instanceof ReadableStream) return value;
  typeError("R2_INVALID_OPTIONS");
}

function objectMeta(meta: R2Metadata) {
  return {
    ...meta,
    uploaded: new Date(meta.uploaded),
    httpMetadata: meta.httpMetadata || {},
    customMetadata: meta.customMetadata || {},
    checksums: meta.md5 ? { md5: meta.md5 } : {},
    storageClass: meta.storageClass || "Standard",
  };
}

export class R2Object {
  declare readonly key: string;
  declare readonly version: string | undefined;
  declare readonly size: number;
  declare readonly etag: string;
  declare readonly httpEtag: string;
  declare readonly uploaded: Date;
  declare readonly httpMetadata: R2HttpMetadata;
  declare readonly customMetadata: Record<string, string>;
  declare readonly range: R2Range | null | undefined;
  declare readonly checksums: { md5?: string };
  declare readonly storageClass: string;

  constructor(meta: R2Metadata) {
    Object.assign(this, objectMeta(meta));
  }

  writeHttpMetadata(headers: Headers) {
    if (!(headers instanceof Headers)) typeError("R2_INVALID_OPTIONS");
    const metadata = this.httpMetadata || {};
    for (const [field, name] of [
      ["contentType", "content-type"],
      ["contentLanguage", "content-language"],
      ["contentDisposition", "content-disposition"],
      ["contentEncoding", "content-encoding"],
      ["cacheControl", "cache-control"],
    ] as const) {
      if (metadata[field] != null) headers.set(name, String(metadata[field]));
    }
    if (metadata.cacheExpiry != null) headers.set("expires", new Date(metadata.cacheExpiry).toUTCString());
  }
}

export class R2ObjectBody extends R2Object {
  constructor(meta: R2Metadata, body: ReadableStream<Uint8Array>) {
    super(meta);
    if (!(body instanceof ReadableStream)) typeError("R2_INTERNAL_PROTOCOL_ERROR");
    const headers = new Headers();
    if (meta.httpMetadata?.contentType != null) {
      headers.set("content-type", String(meta.httpMetadata.contentType));
    }
    const response = new Response(body, { headers });
    bodyState.set(this, { response, claimed: false });
  }

  get body() {
    return bodyState.get(this)!.response.body;
  }

  get bodyUsed() {
    const state = bodyState.get(this)!;
    return state.claimed || state.response.bodyUsed;
  }

  #consume(): Response {
    const state = bodyState.get(this)!;
    if (state.claimed || state.response.bodyUsed) typeError("R2_BODY_ALREADY_USED");
    state.claimed = true;
    return state.response;
  }

  async bytes() { return new Uint8Array(await this.#consume().arrayBuffer()); }
  async arrayBuffer() { return this.#consume().arrayBuffer(); }
  async text() { return this.#consume().text(); }
  async json(): Promise<unknown> { return this.#consume().json(); }
  async blob() { return this.#consume().blob(); }
}

export class R2Bucket {
  constructor(raw: unknown) {
    if (!rawTransport(raw)) typeError("R2_INTERNAL_PROTOCOL_ERROR");
    bucketState.set(this, raw);
  }

  async head(key: string) {
    const meta = await bucketState.get(this)!.head(assertKey(key));
    return meta == null ? null : new R2Object(meta);
  }

  async get(key: string, options: unknown = {}) {
    const input = options == null ? {} : assertObject(options);
    assertKeys(input, ["range", "onlyIf"]);
    const result = await bucketState.get(this)!.get(assertKey(key), {
      range: normalizeRange(input.range),
      onlyIf: normalizeCondition(input.onlyIf),
    });
    if (result == null) return null;
    if (!result.body) return new R2Object(result.meta);
    return new R2ObjectBody(result.meta, result.body);
  }

  async put(key: string, value: unknown, options: unknown = {}) {
    const input = options == null ? {} : assertObject(options);
    assertKeys(input, [
      "onlyIf",
      "httpMetadata",
      "customMetadata",
      "md5",
      "storageClass",
    ]);
    if (input.storageClass != null && input.storageClass !== "Standard") {
      typeError("R2_UNSUPPORTED_FEATURE");
    }
    const onlyIf = normalizeCondition(input.onlyIf);
    if (onlyIf && (onlyIf.uploadedBefore != null || onlyIf.uploadedAfter != null)) {
      typeError("R2_UNSUPPORTED_CONDITION");
    }
    const meta = await bucketState.get(this)!.put(assertKey(key), putBody(value), {
      onlyIf,
      httpMetadata: normalizeHttpMetadata(input.httpMetadata),
      customMetadata: normalizeCustomMetadata(input.customMetadata),
      md5: normalizeMd5(input.md5),
      storageClass: "Standard",
    });
    return meta == null ? null : new R2Object(meta);
  }

  async delete(keys: string | string[]) {
    const values = Array.isArray(keys) ? keys.map(assertKey) : [assertKey(keys)];
    await bucketState.get(this)!.delete(values);
  }

  async list(options: unknown = {}) {
    const input = options == null ? {} : assertObject(options);
    assertKeys(input, ["prefix", "delimiter", "cursor", "limit", "include"]);
    if (input.include != null && !Array.isArray(input.include)) typeError("R2_INVALID_OPTIONS");
    const include = input.include == null ? [] : input.include.map(String);
    const limit = input.limit == null ? 1000 : safeInteger(input.limit);
    if (limit < 1 || limit > 1000) typeError("R2_INVALID_OPTIONS");
    const result = await bucketState.get(this)!.list({
      prefix: input.prefix == null ? "" : assertKey(input.prefix),
      delimiter: input.delimiter == null ? undefined : String(input.delimiter),
      cursor: input.cursor == null ? undefined : String(input.cursor),
      limit,
      include,
    });
    return { ...result, objects: (result.objects || []).map((meta) => new R2Object(meta)) };
  }

  createMultipartUpload() { typeError("R2_UNSUPPORTED_FEATURE"); }
  resumeMultipartUpload() { typeError("R2_UNSUPPORTED_FEATURE"); }
}

function rawTransport(raw: unknown): raw is R2RawTransport {
  return raw !== null && typeof raw === "object"
    && "head" in raw && typeof raw.head === "function"
    && "get" in raw && typeof raw.get === "function"
    && "put" in raw && typeof raw.put === "function"
    && "delete" in raw && typeof raw.delete === "function"
    && "list" in raw && typeof raw.list === "function";
}

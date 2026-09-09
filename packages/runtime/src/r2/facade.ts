import type {
  R2Checksums as R2ChecksumsWire,
  R2HttpMetadata,
  R2Metadata,
  R2MultipartCreateOptions,
  R2Range,
  R2RawTransport,
  R2UploadedPart,
} from "./protocol.js";
import {
  assertKey,
  assertObject,
  bytesFromHex,
  hexFromBytes,
  int32,
  normalizeCondition,
  normalizeCustomMetadata,
  normalizeHttpMetadata,
  normalizeRange,
  parseChecksumOption,
  parseSsecKey,
  parseStorageClass,
  putBody,
  typeError,
} from "./validation.js";

const bucketState = new WeakMap<object, R2RawTransport>();
const bodyState = new WeakMap<
  object,
  { response: Response; claimed: boolean }
>();
function checksumsFromWire(wire: R2ChecksumsWire | undefined): R2Checksums {
  return new R2Checksums(wire || {});
}

function httpMetadataFromWire(meta: R2HttpMetadata | null | undefined) {
  if (meta == null) return undefined;
  const out: Record<string, unknown> = {};
  for (const name of [
    "contentType",
    "contentLanguage",
    "contentDisposition",
    "contentEncoding",
    "cacheControl",
  ] as const) {
    if (meta[name] != null) out[name] = String(meta[name]);
  }
  if (meta.cacheExpiry != null) out.cacheExpiry = new Date(meta.cacheExpiry);
  return out;
}

function rangeFromWire(range: R2Range | null | undefined): R2Range | undefined {
  if (range == null) return undefined;
  const out: R2Range = {};
  if (range.offset != null) out.offset = range.offset;
  if (range.length != null) out.length = range.length;
  if (range.suffix != null) out.suffix = range.suffix;
  return out;
}

function objectFields(meta: R2Metadata) {
  return {
    key: meta.key,
    version: meta.version,
    size: meta.size,
    etag: meta.etag,
    httpEtag: meta.httpEtag,
    uploaded: new Date(meta.uploaded),
    httpMetadata: httpMetadataFromWire(meta.httpMetadata ?? undefined),
    customMetadata:
      meta.customMetadata == null ? undefined : { ...meta.customMetadata },
    range: rangeFromWire(meta.range),
    checksums: checksumsFromWire(meta.checksums),
    storageClass: meta.storageClass,
    ssecKeyMd5: meta.ssecKeyMd5 || undefined,
  };
}

export class R2Checksums {
  declare readonly md5?: ArrayBuffer;
  declare readonly sha1?: ArrayBuffer;
  declare readonly sha256?: ArrayBuffer;
  declare readonly sha384?: ArrayBuffer;
  declare readonly sha512?: ArrayBuffer;

  constructor(wire: R2ChecksumsWire) {
    const assign = (name: keyof R2ChecksumsWire, hex: string | undefined) => {
      if (hex)
        Object.defineProperty(this, name, {
          value: bytesFromHex(hex),
          enumerable: true,
        });
    };
    assign("md5", wire.md5);
    assign("sha1", wire.sha1);
    assign("sha256", wire.sha256);
    assign("sha384", wire.sha384);
    assign("sha512", wire.sha512);
  }

  toJSON() {
    const json: R2ChecksumsWire = {};
    if (this.md5) json.md5 = hexFromBytes(new Uint8Array(this.md5));
    if (this.sha1) json.sha1 = hexFromBytes(new Uint8Array(this.sha1));
    if (this.sha256) json.sha256 = hexFromBytes(new Uint8Array(this.sha256));
    if (this.sha384) json.sha384 = hexFromBytes(new Uint8Array(this.sha384));
    if (this.sha512) json.sha512 = hexFromBytes(new Uint8Array(this.sha512));
    return json;
  }
}

export class R2Object {
  declare readonly key: string;
  declare readonly version: string;
  declare readonly size: number;
  declare readonly etag: string;
  declare readonly httpEtag: string;
  declare readonly checksums: R2Checksums;
  declare readonly uploaded: Date;
  declare readonly httpMetadata?: R2HttpMetadata & { cacheExpiry?: Date };
  declare readonly customMetadata?: Record<string, string>;
  declare readonly range?: R2Range;
  declare readonly storageClass: string;
  declare readonly ssecKeyMd5?: string;

  constructor(meta: R2Metadata) {
    Object.assign(this, objectFields(meta));
  }

  writeHttpMetadata(headers: Headers) {
    if (!(headers instanceof Headers)) typeError("R2_INVALID_OPTIONS");
    const metadata = this.httpMetadata;
    if (metadata == null) {
      typeError(
        `HTTP metadata unknown for key \`${this.key}\`. Did you forget to add 'httpMetadata' to \`include\` when listing?`,
      );
    }
    for (const [field, name] of [
      ["contentType", "content-type"],
      ["contentLanguage", "content-language"],
      ["contentDisposition", "content-disposition"],
      ["contentEncoding", "content-encoding"],
      ["cacheControl", "cache-control"],
    ] as const) {
      if (metadata[field] != null) headers.set(name, String(metadata[field]));
    }
    if (metadata.cacheExpiry != null) {
      headers.set(
        "expires",
        new Date(metadata.cacheExpiry as Date | number).toUTCString(),
      );
    }
  }
}

export class R2ObjectBody extends R2Object {
  constructor(meta: R2Metadata, body: ReadableStream<Uint8Array>) {
    super(meta);
    if (!(body instanceof ReadableStream))
      typeError("R2_INTERNAL_PROTOCOL_ERROR");
    const headers = new Headers();
    if (meta.httpMetadata?.contentType != null)
      headers.set("content-type", String(meta.httpMetadata.contentType));
    bodyState.set(this, {
      response: new Response(body, { headers }),
      claimed: false,
    });
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
    if (state.claimed || state.response.bodyUsed) {
      typeError(
        "Body has already been used. It can only be used once. Use tee() first if you need to read it twice.",
      );
    }
    state.claimed = true;
    return state.response;
  }

  async bytes() {
    return new Uint8Array(await this.#consume().arrayBuffer());
  }
  async arrayBuffer() {
    return this.#consume().arrayBuffer();
  }
  async text() {
    return this.#consume().text();
  }
  async json(): Promise<unknown> {
    return this.#consume().json();
  }
  async blob() {
    return this.#consume().blob();
  }
}

export class R2MultipartUpload {
  declare readonly key: string;
  declare readonly uploadId: string;

  constructor(bucket: R2RawTransport, key: string, uploadId: string) {
    Object.assign(this, { key, uploadId });
    bucketState.set(this, bucket);
  }

  async uploadPart(partNumber: number, value: unknown, options: unknown = {}) {
    const number = int32(partNumber);
    if (number < 1 || number > 10000) {
      typeError(
        `Part number must be between 1 and 10000 (inclusive). Actual value was: ${number}`,
      );
    }
    const input = options == null ? {} : assertObject(options);
    return bucketState
      .get(this)!
      .uploadPart(
        this.key,
        this.uploadId,
        number,
        putBody(value),
        parseSsecKey(input.ssecKey),
      );
  }

  async abort() {
    await bucketState.get(this)!.abortMultipartUpload(this.key, this.uploadId);
  }

  async complete(uploadedParts: R2UploadedPart[]) {
    if (!Array.isArray(uploadedParts)) {
      typeError(
        "Failed to execute 'complete' on 'R2MultipartUpload': parameter 1 is not of type 'Array'.",
      );
    }
    const parts = uploadedParts.map((part) => {
      const input = assertObject(part);
      const partNumber = int32(input.partNumber);
      if (partNumber < 1 || partNumber > 10000) {
        typeError(
          `Part number must be between 1 and 10000 (inclusive). Actual value was: ${partNumber}`,
        );
      }
      if (typeof input.etag !== "string") typeError("R2_INVALID_OPTIONS");
      return { partNumber, etag: input.etag };
    });
    const metadata = await bucketState
      .get(this)!
      .completeMultipartUpload(this.key, this.uploadId, parts);
    const completed = { ...metadata };
    delete completed.httpMetadata;
    delete completed.customMetadata;
    return new R2Object(completed);
  }
}

function multipartOptions(options: unknown): R2MultipartCreateOptions {
  const input = options == null ? {} : assertObject(options);
  const ssecKey = parseSsecKey(input.ssecKey);
  const storageClass = parseStorageClass(
    input.storageClass,
    "createMultipartUpload",
  );
  return {
    httpMetadata: normalizeHttpMetadata(input.httpMetadata),
    customMetadata: normalizeCustomMetadata(input.customMetadata),
    ...(storageClass ? { storageClass } : {}),
    ...(ssecKey ? { ssecKey } : {}),
  };
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
    const range = normalizeRange(input.range);
    const onlyIf = normalizeCondition(input.onlyIf);
    const ssecKey = parseSsecKey(input.ssecKey);
    const result = await bucketState.get(this)!.get(assertKey(key), {
      ...(range ? { range } : {}),
      ...(onlyIf ? { onlyIf } : {}),
      ...(ssecKey ? { ssecKey } : {}),
    });
    if (result == null) return null;
    if (!result.body) return new R2Object(result.meta);
    return new R2ObjectBody(result.meta, result.body);
  }

  async put(key: string, value: unknown, options: unknown = {}) {
    const input = options == null ? {} : assertObject(options);
    const onlyIf = normalizeCondition(input.onlyIf);
    const checksum = parseChecksumOption(input);
    const ssecKey = parseSsecKey(input.ssecKey);
    const storageClass = parseStorageClass(input.storageClass, "put");
    const meta = await bucketState
      .get(this)!
      .put(assertKey(key), putBody(value), {
        httpMetadata: normalizeHttpMetadata(input.httpMetadata),
        customMetadata: normalizeCustomMetadata(input.customMetadata),
        ...(storageClass ? { storageClass } : {}),
        ...(onlyIf ? { onlyIf } : {}),
        ...(checksum ? { checksum } : {}),
        ...(ssecKey ? { ssecKey } : {}),
      });
    return meta == null ? null : new R2Object(meta);
  }

  async delete(keys: string | string[]) {
    const values = Array.isArray(keys)
      ? keys.map(assertKey)
      : [assertKey(keys)];
    if (values.length > 1000) typeError("R2_INVALID_OPTIONS");
    await bucketState.get(this)!.delete(values);
  }

  async list(options: unknown = {}) {
    const input = options == null ? {} : assertObject(options);
    if (input.include != null && !Array.isArray(input.include))
      typeError("R2_INVALID_OPTIONS");
    const include =
      input.include == null
        ? []
        : input.include
            .map((item) => {
              if (typeof item !== "string") typeError("R2_INVALID_OPTIONS");
              const value = item;
              if (value !== "httpMetadata" && value !== "customMetadata") {
                throw new RangeError(`Unsupported include value ${value}`);
              }
              return value;
            })
            .filter((value, index, values) => values.indexOf(value) === index);
    const requestedLimit = input.limit == null ? 1000 : int32(input.limit);
    const limit =
      requestedLimit < 0 || requestedLimit > 1000 ? 1000 : requestedLimit;
    const delimiter = input.delimiter == null ? undefined : input.delimiter;
    const cursor = input.cursor == null ? undefined : input.cursor;
    const startAfter = input.startAfter == null ? undefined : input.startAfter;
    if (delimiter !== undefined && typeof delimiter !== "string")
      typeError("R2_INVALID_OPTIONS");
    if (cursor !== undefined && typeof cursor !== "string")
      typeError("R2_INVALID_OPTIONS");
    if (startAfter !== undefined && typeof startAfter !== "string")
      typeError("R2_INVALID_OPTIONS");
    if (input.prefix != null && typeof input.prefix !== "string")
      typeError("R2_INVALID_OPTIONS");
    const result = await bucketState.get(this)!.list({
      prefix: input.prefix == null ? "" : input.prefix,
      limit,
      include,
      ...(delimiter ? { delimiter } : {}),
      ...(cursor ? { cursor } : {}),
      ...(startAfter ? { startAfter } : {}),
    });
    const objects = (result.objects || []).map((meta) => new R2Object(meta));
    if (result.truncated)
      return {
        objects,
        truncated: true,
        cursor: result.cursor,
        delimitedPrefixes: result.delimitedPrefixes || [],
      };
    return {
      objects,
      truncated: false,
      cursor: undefined,
      delimitedPrefixes: result.delimitedPrefixes || [],
    };
  }

  async createMultipartUpload(key: string, options: unknown = {}) {
    const created = await bucketState
      .get(this)!
      .createMultipartUpload(assertKey(key), multipartOptions(options));
    return new R2MultipartUpload(
      bucketState.get(this)!,
      created.key,
      created.uploadId,
    );
  }

  resumeMultipartUpload(key: string, uploadId: string) {
    return new R2MultipartUpload(
      bucketState.get(this)!,
      assertKey(key),
      assertKey(uploadId),
    );
  }
}

function rawTransport(raw: unknown): raw is R2RawTransport {
  return (
    raw !== null &&
    typeof raw === "object" &&
    "head" in raw &&
    typeof raw.head === "function" &&
    "get" in raw &&
    typeof raw.get === "function" &&
    "put" in raw &&
    typeof raw.put === "function" &&
    "delete" in raw &&
    typeof raw.delete === "function" &&
    "list" in raw &&
    typeof raw.list === "function" &&
    "createMultipartUpload" in raw &&
    typeof raw.createMultipartUpload === "function" &&
    "uploadPart" in raw &&
    typeof raw.uploadPart === "function" &&
    "completeMultipartUpload" in raw &&
    typeof raw.completeMultipartUpload === "function" &&
    "abortMultipartUpload" in raw &&
    typeof raw.abortMultipartUpload === "function"
  );
}

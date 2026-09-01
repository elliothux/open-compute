import type {
  R2Checksums as R2ChecksumsWire,
  R2Condition,
  R2EtagMatch,
  R2HttpMetadata,
  R2Metadata,
  R2MultipartCreateOptions,
  R2PutOptions as R2PutWireOptions,
  R2Range,
  R2RawTransport,
  R2UploadedPart,
} from "./protocol.js";

const bucketState = new WeakMap<object, R2RawTransport>();
const bodyState = new WeakMap<object, { response: Response; claimed: boolean }>();
const encoder = new TextEncoder();

function typeError(code: string): never {
  throw new TypeError(code);
}

function assertObject(value: unknown): Record<string, unknown> {
  if (value === null || (typeof value !== "object" && typeof value !== "function")) {
    typeError("R2_INVALID_OPTIONS");
  }
  return value as Record<string, unknown>;
}

function domString(value: unknown): string {
  if (typeof value === "symbol") throw new TypeError("Cannot convert a Symbol value to a string");
  return `${value}`;
}

function assertKey(value: unknown): string {
  const input = domString(value);
  const output: string[] = [];
  for (let index = 0; index < input.length; index++) {
    const code = input.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = input.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        output.push(input[index]!, input[++index]!);
      } else {
        output.push("\ufffd\ufffd\ufffd");
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      output.push("\ufffd\ufffd\ufffd");
    } else {
      output.push(input[index]!);
    }
  }
  return output.join("");
}

function int32(value: unknown): number {
  return Number(value) >> 0;
}

function dateMillis(value: unknown, header = false): number {
  if (!(value instanceof Date) && !(header && typeof value === "string")) {
    typeError("R2_INVALID_OPTIONS");
  }
  const millis = (value instanceof Date ? value : new Date(value as string)).getTime();
  if (!Number.isFinite(millis)) typeError("R2_INVALID_OPTIONS");
  return millis;
}

function isQuotedEtag(value: string): boolean {
  return value.startsWith("\"") && value.endsWith("\"");
}

function parseConditionalEtags(header: string): R2EtagMatch[] {
  const out: R2EtagMatch[] = [];
  let rest = header;
  let needComma = false;
  for (;;) {
    rest = rest.replace(/^[ \t]+/, "");
    if (!rest) return out;
    if (rest.startsWith(",")) {
      rest = rest.slice(1);
      needComma = false;
      continue;
    }
    if (rest.startsWith("*")) {
      if (needComma) {
        throw new Error("Comma was expected to separate etags. Encountered a wildcard character '*' instead.");
      }
      return [{ kind: "wildcard" }];
    }
    if (rest.startsWith("W/")) {
      if (needComma) {
        throw new Error("Comma was expected to separate etags. Encountered a weak quotation character 'W' instead. This would otherwise indicate the start of a new weak etag.");
      }
      if (rest.length < 3 || rest[2] !== "\"") {
        throw new Error("Weak etags must start with W/ and their value must be quoted");
      }
      rest = rest.slice(3);
      const end = rest.indexOf("\"");
      if (end < 0) throw new Error("Unclosed double quote for Etag");
      out.push({ kind: "weak", value: rest.slice(0, end) });
      rest = rest.slice(end + 1);
      needComma = true;
      continue;
    }
    if (rest.startsWith("\"")) {
      if (needComma) {
        throw new Error("Comma was expected to separate etags. Encountered a double quote character '\"' instead. This would otherwise indicate the start of a new strong etag.");
      }
      rest = rest.slice(1);
      const end = rest.indexOf("\"");
      if (end < 0) throw new Error("Unclosed double quote for Etag");
      out.push({ kind: "strong", value: rest.slice(0, end) });
      rest = rest.slice(end + 1);
      needComma = true;
      continue;
    }
    return out;
  }
}

function normalizeCondition(value: unknown): R2Condition | undefined {
  if (value == null) return undefined;
  if (value instanceof Headers) {
    const matches = value.get("if-match");
    const differs = value.get("if-none-match");
    const before = value.get("if-unmodified-since");
    const after = value.get("if-modified-since");
    const etagMatches = matches ? parseConditionalEtags(matches) : [];
    const etagDoesNotMatch = differs ? parseConditionalEtags(differs) : [];
    if (matches && etagMatches.length === 0) throw new Error("Invalid ETag in if-match header");
    if (differs && etagDoesNotMatch.length === 0) throw new Error("Invalid ETag in if-none-match header");
    return {
      etagMatches,
      etagDoesNotMatch,
      secondsGranularity: true,
      httpHeaders: true,
      ...(before ? { uploadedBefore: dateMillis(before, true) } : {}),
      ...(after ? { uploadedAfter: dateMillis(after, true) } : {}),
    };
  }
  const input = assertObject(value);
  if (input.etagMatches != null && typeof input.etagMatches !== "string") typeError("R2_INVALID_OPTIONS");
  if (input.etagDoesNotMatch != null && typeof input.etagDoesNotMatch !== "string") typeError("R2_INVALID_OPTIONS");
  if (typeof input.etagMatches === "string" && isQuotedEtag(input.etagMatches)) {
    typeError(`Conditional ETag should not be wrapped in quotes (${input.etagMatches}).`);
  }
  if (typeof input.etagDoesNotMatch === "string" && isQuotedEtag(input.etagDoesNotMatch)) {
    typeError(`Conditional ETag should not be wrapped in quotes (${input.etagDoesNotMatch}).`);
  }
  return {
    etagMatches: input.etagMatches == null ? [] : input.etagMatches === "*"
      ? [{ kind: "wildcard" as const }]
      : [{ kind: "strong" as const, value: String(input.etagMatches) }],
    etagDoesNotMatch: input.etagDoesNotMatch == null ? [] : input.etagDoesNotMatch === "*"
      ? [{ kind: "wildcard" as const }]
      : [{ kind: "strong" as const, value: String(input.etagDoesNotMatch) }],
    secondsGranularity: input.secondsGranularity == null ? false : Boolean(input.secondsGranularity),
    httpHeaders: false,
    ...(input.uploadedBefore == null ? {} : { uploadedBefore: dateMillis(input.uploadedBefore) }),
    ...(input.uploadedAfter == null ? {} : { uploadedAfter: dateMillis(input.uploadedAfter) }),
  };
}

function normalizeRange(value: unknown): R2Range | undefined {
  if (value == null) return undefined;
  if (value instanceof Headers) {
    const header = value.get("range");
    if (!header) return undefined;
    if (header.includes(",")) return undefined;
    const match = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
    if (!match || (!match[1] && !match[2])) return undefined;
    if (!match[1]) return { suffix: Number(match[2]) };
    const offset = Number(match[1]);
    if (!match[2]) return { offset };
    const end = Number(match[2]);
    if (end < offset) return undefined;
    return { offset, length: end - offset + 1 };
  }
  const input = assertObject(value);
  const out: R2Range = {};
  if (input.offset != null) {
    const offset = Number(input.offset);
    if (offset < 0) throw new RangeError(`Invalid range. Starting offset (${offset}) must be greater than or equal to 0.`);
    if (!Number.isInteger(offset)) {
      throw new RangeError(`Invalid range. Starting offset (${offset}) must be an integer, not floating point.`);
    }
    out.offset = offset;
  }
  if (input.length != null) {
    const length = Number(input.length);
    if (length < 0) throw new RangeError(`Invalid range. Length (${length}) must be greater than or equal to 0.`);
    if (!Number.isInteger(length)) {
      throw new RangeError(`Invalid range. Length (${length}) must be an integer, not floating point.`);
    }
    if (length === 0) throw new Error("get: The requested range is not satisfiable (10039)");
    out.length = length;
  }
  if (input.suffix != null) {
    if (out.offset != null) typeError("Suffix is incompatible with offset.");
    if (out.length != null) typeError("Suffix is incompatible with length.");
    const suffix = Number(input.suffix);
    if (suffix < 0) throw new RangeError(`Invalid suffix. Suffix (${suffix}) must be greater than or equal to 0.`);
    if (!Number.isInteger(suffix)) {
      throw new RangeError(`Invalid range. Suffix (${suffix}) must be an integer, not floating point.`);
    }
    if (suffix === 0) throw new Error("get: The requested range is not satisfiable (10039)");
    out.suffix = suffix;
  }
  if (out.offset == null && out.length == null && out.suffix == null) {
    throw new Error("get: We encountered an internal error. Please try again. (10001)");
  }
  return out;
}

function normalizeHttpMetadata(value: unknown): R2HttpMetadata {
  if (value == null) return {};
  if (value instanceof Headers) {
    const expires = value.get("expires");
    const out: R2HttpMetadata = {};
    const contentType = value.get("content-type");
    const contentLanguage = value.get("content-language");
    const contentDisposition = value.get("content-disposition");
    const contentEncoding = value.get("content-encoding");
    const cacheControl = value.get("cache-control");
    if (contentType) out.contentType = contentType;
    if (contentLanguage) out.contentLanguage = contentLanguage;
    if (contentDisposition) out.contentDisposition = contentDisposition;
    if (contentEncoding) out.contentEncoding = contentEncoding;
    if (cacheControl) out.cacheControl = cacheControl;
    if (expires) out.cacheExpiry = dateMillis(expires, true);
    return out;
  }
  const input = assertObject(value);
  const out: R2HttpMetadata = {};
  for (const name of ["contentType", "contentLanguage", "contentDisposition", "contentEncoding", "cacheControl"] as const) {
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

function hexFromBytes(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function bytesFromHex(hex: string): ArrayBuffer {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes.buffer;
}

function parseChecksum(value: unknown, name: string, bytes: number, hexChars: number): string {
  if (typeof value === "string") {
    if (value.length !== hexChars) typeError(`${name} is ${hexChars} hex characters, not ${value.length}`);
    const decoded = value.replace(/[^0-9a-fA-F]/g, "");
    if (decoded.length !== hexChars) typeError(`Provided ${name} wasn't a valid hex string`);
    return decoded.toLowerCase();
  }
  let view: Uint8Array | undefined;
  if (value instanceof ArrayBuffer) view = new Uint8Array(value);
  else if (ArrayBuffer.isView(value)) view = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (!view) typeError("R2_INVALID_OPTIONS");
  if (view.byteLength !== bytes) typeError(`${name} is ${bytes} bytes, not ${view.byteLength}`);
  return hexFromBytes(view);
}

function parseSsecKey(value: unknown): string | undefined {
  if (value == null) return undefined;
  if (typeof value === "string") {
    if (!/^[0-9a-f]+$/.test(value)) throw new Error("SSE-C Key has invalid format");
    if (value.length !== 64) throw new Error("SSE-C Key must be 32 bytes in length");
    return value;
  }
  let view: Uint8Array | undefined;
  if (value instanceof ArrayBuffer) view = new Uint8Array(value);
  else if (ArrayBuffer.isView(value)) view = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (!view || view.byteLength !== 32) throw new Error("SSE-C Key must be 32 bytes in length");
  return hexFromBytes(view);
}

function parseStorageClass(value: unknown, operation: "put" | "createMultipartUpload"): string | undefined {
  if (value == null) return undefined;
  const storageClass = domString(value);
  if (storageClass !== "Standard" && storageClass !== "InfrequentAccess") {
    throw new Error(`${operation}: We encountered an internal error. Please try again. (10001)`);
  }
  return storageClass;
}

function parseChecksumOption(input: Record<string, unknown>): R2PutWireOptions["checksum"] {
  const algorithms = [
    ["md5", "MD5", 16, 32],
    ["sha1", "SHA-1", 20, 40],
    ["sha256", "SHA-256", 32, 64],
    ["sha384", "SHA-384", 48, 96],
    ["sha512", "SHA-512", 64, 128],
  ] as const;
  let found: R2PutWireOptions["checksum"];
  for (const [name, label, bytes, hex] of algorithms) {
    if (input[name] == null) continue;
    if (found) typeError("You cannot specify multiple hashing algorithms.");
    found = { algorithm: name, hex: parseChecksum(input[name], label, bytes, hex) };
  }
  return found;
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

function checksumsFromWire(wire: R2ChecksumsWire | undefined): R2Checksums {
  return new R2Checksums(wire || {});
}

function httpMetadataFromWire(meta: R2HttpMetadata | null | undefined) {
  if (meta == null) return undefined;
  const out: Record<string, unknown> = {};
  for (const name of ["contentType", "contentLanguage", "contentDisposition", "contentEncoding", "cacheControl"] as const) {
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
    customMetadata: meta.customMetadata == null ? undefined : { ...meta.customMetadata },
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
      if (hex) Object.defineProperty(this, name, { value: bytesFromHex(hex), enumerable: true });
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
      typeError(`HTTP metadata unknown for key \`${this.key}\`. Did you forget to add 'httpMetadata' to \`include\` when listing?`);
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
      headers.set("expires", new Date(metadata.cacheExpiry as Date | number).toUTCString());
    }
  }
}

export class R2ObjectBody extends R2Object {
  constructor(meta: R2Metadata, body: ReadableStream<Uint8Array>) {
    super(meta);
    if (!(body instanceof ReadableStream)) typeError("R2_INTERNAL_PROTOCOL_ERROR");
    const headers = new Headers();
    if (meta.httpMetadata?.contentType != null) headers.set("content-type", String(meta.httpMetadata.contentType));
    bodyState.set(this, { response: new Response(body, { headers }), claimed: false });
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
      typeError("Body has already been used. It can only be used once. Use tee() first if you need to read it twice.");
    }
    state.claimed = true;
    return state.response;
  }

  async bytes() { return new Uint8Array(await this.#consume().arrayBuffer()); }
  async arrayBuffer() { return this.#consume().arrayBuffer(); }
  async text() { return this.#consume().text(); }
  async json(): Promise<unknown> { return this.#consume().json(); }
  async blob() { return this.#consume().blob(); }
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
      typeError(`Part number must be between 1 and 10000 (inclusive). Actual value was: ${number}`);
    }
    const input = options == null ? {} : assertObject(options);
    return bucketState.get(this)!.uploadPart(
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
      typeError("Failed to execute 'complete' on 'R2MultipartUpload': parameter 1 is not of type 'Array'.");
    }
    const parts = uploadedParts.map((part) => {
      const input = assertObject(part);
      const partNumber = int32(input.partNumber);
      if (partNumber < 1 || partNumber > 10000) {
        typeError(`Part number must be between 1 and 10000 (inclusive). Actual value was: ${partNumber}`);
      }
      if (typeof input.etag !== "string") typeError("R2_INVALID_OPTIONS");
      return { partNumber, etag: input.etag };
    });
    const metadata = await bucketState.get(this)!.completeMultipartUpload(this.key, this.uploadId, parts);
    const completed = { ...metadata };
    delete completed.httpMetadata;
    delete completed.customMetadata;
    return new R2Object(completed);
  }
}

function multipartOptions(options: unknown): R2MultipartCreateOptions {
  const input = options == null ? {} : assertObject(options);
  const ssecKey = parseSsecKey(input.ssecKey);
  const storageClass = parseStorageClass(input.storageClass, "createMultipartUpload");
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
    const meta = await bucketState.get(this)!.put(assertKey(key), putBody(value), {
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
    const values = Array.isArray(keys) ? keys.map(assertKey) : [assertKey(keys)];
    if (values.length > 1000) typeError("R2_INVALID_OPTIONS");
    await bucketState.get(this)!.delete(values);
  }

  async list(options: unknown = {}) {
    const input = options == null ? {} : assertObject(options);
    if (input.include != null && !Array.isArray(input.include)) typeError("R2_INVALID_OPTIONS");
    const include = input.include == null ? [] : input.include.map((item) => {
      if (typeof item !== "string") typeError("R2_INVALID_OPTIONS");
      const value = item;
      if (value !== "httpMetadata" && value !== "customMetadata") {
        throw new RangeError(`Unsupported include value ${value}`);
      }
      return value;
    }).filter((value, index, values) => values.indexOf(value) === index);
    const requestedLimit = input.limit == null ? 1000 : int32(input.limit);
    const limit = requestedLimit < 0 || requestedLimit > 1000 ? 1000 : requestedLimit;
    const delimiter = input.delimiter == null ? undefined : input.delimiter;
    const cursor = input.cursor == null ? undefined : input.cursor;
    const startAfter = input.startAfter == null ? undefined : input.startAfter;
    if (delimiter !== undefined && typeof delimiter !== "string") typeError("R2_INVALID_OPTIONS");
    if (cursor !== undefined && typeof cursor !== "string") typeError("R2_INVALID_OPTIONS");
    if (startAfter !== undefined && typeof startAfter !== "string") typeError("R2_INVALID_OPTIONS");
    if (input.prefix != null && typeof input.prefix !== "string") typeError("R2_INVALID_OPTIONS");
    const result = await bucketState.get(this)!.list({
      prefix: input.prefix == null ? "" : input.prefix,
      limit,
      include,
      ...(delimiter ? { delimiter } : {}),
      ...(cursor ? { cursor } : {}),
      ...(startAfter ? { startAfter } : {}),
    });
    const objects = (result.objects || []).map((meta) => new R2Object(meta));
    if (result.truncated) return { objects, truncated: true, cursor: result.cursor, delimitedPrefixes: result.delimitedPrefixes || [] };
    return { objects, truncated: false, cursor: undefined, delimitedPrefixes: result.delimitedPrefixes || [] };
  }

  async createMultipartUpload(key: string, options: unknown = {}) {
    const created = await bucketState.get(this)!.createMultipartUpload(assertKey(key), multipartOptions(options));
    return new R2MultipartUpload(bucketState.get(this)!, created.key, created.uploadId);
  }

  resumeMultipartUpload(key: string, uploadId: string) {
    return new R2MultipartUpload(bucketState.get(this)!, assertKey(key), assertKey(uploadId));
  }
}

function rawTransport(raw: unknown): raw is R2RawTransport {
  return raw !== null && typeof raw === "object"
    && "head" in raw && typeof raw.head === "function"
    && "get" in raw && typeof raw.get === "function"
    && "put" in raw && typeof raw.put === "function"
    && "delete" in raw && typeof raw.delete === "function"
    && "list" in raw && typeof raw.list === "function"
    && "createMultipartUpload" in raw && typeof raw.createMultipartUpload === "function"
    && "uploadPart" in raw && typeof raw.uploadPart === "function"
    && "completeMultipartUpload" in raw && typeof raw.completeMultipartUpload === "function"
    && "abortMultipartUpload" in raw && typeof raw.abortMultipartUpload === "function";
}

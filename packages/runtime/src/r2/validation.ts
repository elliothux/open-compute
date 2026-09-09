import type {
  R2Condition,
  R2EtagMatch,
  R2HttpMetadata,
  R2PutOptions as R2PutWireOptions,
  R2Range,
} from "./protocol.js";

const encoder = new TextEncoder();

export function typeError(code: string): never {
  throw new TypeError(code);
}

export function assertObject(value: unknown): Record<string, unknown> {
  if (
    value === null ||
    (typeof value !== "object" && typeof value !== "function")
  ) {
    typeError("R2_INVALID_OPTIONS");
  }
  return value as Record<string, unknown>;
}

function domString(value: unknown): string {
  if (typeof value === "symbol")
    throw new TypeError("Cannot convert a Symbol value to a string");
  return `${value}`;
}

export function assertKey(value: unknown): string {
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

export function int32(value: unknown): number {
  return Number(value) >> 0;
}

function dateMillis(value: unknown, header = false): number {
  if (!(value instanceof Date) && !(header && typeof value === "string")) {
    typeError("R2_INVALID_OPTIONS");
  }
  const millis = (
    value instanceof Date ? value : new Date(value as string)
  ).getTime();
  if (!Number.isFinite(millis)) typeError("R2_INVALID_OPTIONS");
  return millis;
}

function isQuotedEtag(value: string): boolean {
  return value.startsWith('"') && value.endsWith('"');
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
        throw new Error(
          "Comma was expected to separate etags. Encountered a wildcard character '*' instead.",
        );
      }
      return [{ kind: "wildcard" }];
    }
    if (rest.startsWith("W/")) {
      if (needComma) {
        throw new Error(
          "Comma was expected to separate etags. Encountered a weak quotation character 'W' instead. This would otherwise indicate the start of a new weak etag.",
        );
      }
      if (rest.length < 3 || rest[2] !== '"') {
        throw new Error(
          "Weak etags must start with W/ and their value must be quoted",
        );
      }
      rest = rest.slice(3);
      const end = rest.indexOf('"');
      if (end < 0) throw new Error("Unclosed double quote for Etag");
      out.push({ kind: "weak", value: rest.slice(0, end) });
      rest = rest.slice(end + 1);
      needComma = true;
      continue;
    }
    if (rest.startsWith('"')) {
      if (needComma) {
        throw new Error(
          "Comma was expected to separate etags. Encountered a double quote character '\"' instead. This would otherwise indicate the start of a new strong etag.",
        );
      }
      rest = rest.slice(1);
      const end = rest.indexOf('"');
      if (end < 0) throw new Error("Unclosed double quote for Etag");
      out.push({ kind: "strong", value: rest.slice(0, end) });
      rest = rest.slice(end + 1);
      needComma = true;
      continue;
    }
    return out;
  }
}

export function normalizeCondition(value: unknown): R2Condition | undefined {
  if (value == null) return undefined;
  if (value instanceof Headers) {
    const matches = value.get("if-match");
    const differs = value.get("if-none-match");
    const before = value.get("if-unmodified-since");
    const after = value.get("if-modified-since");
    const etagMatches = matches ? parseConditionalEtags(matches) : [];
    const etagDoesNotMatch = differs ? parseConditionalEtags(differs) : [];
    if (matches && etagMatches.length === 0)
      throw new Error("Invalid ETag in if-match header");
    if (differs && etagDoesNotMatch.length === 0)
      throw new Error("Invalid ETag in if-none-match header");
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
  if (input.etagMatches != null && typeof input.etagMatches !== "string")
    typeError("R2_INVALID_OPTIONS");
  if (
    input.etagDoesNotMatch != null &&
    typeof input.etagDoesNotMatch !== "string"
  )
    typeError("R2_INVALID_OPTIONS");
  if (
    typeof input.etagMatches === "string" &&
    isQuotedEtag(input.etagMatches)
  ) {
    typeError(
      `Conditional ETag should not be wrapped in quotes (${input.etagMatches}).`,
    );
  }
  if (
    typeof input.etagDoesNotMatch === "string" &&
    isQuotedEtag(input.etagDoesNotMatch)
  ) {
    typeError(
      `Conditional ETag should not be wrapped in quotes (${input.etagDoesNotMatch}).`,
    );
  }
  return {
    etagMatches:
      input.etagMatches == null
        ? []
        : input.etagMatches === "*"
          ? [{ kind: "wildcard" as const }]
          : [{ kind: "strong" as const, value: String(input.etagMatches) }],
    etagDoesNotMatch:
      input.etagDoesNotMatch == null
        ? []
        : input.etagDoesNotMatch === "*"
          ? [{ kind: "wildcard" as const }]
          : [
              {
                kind: "strong" as const,
                value: String(input.etagDoesNotMatch),
              },
            ],
    secondsGranularity:
      input.secondsGranularity == null
        ? false
        : Boolean(input.secondsGranularity),
    httpHeaders: false,
    ...(input.uploadedBefore == null
      ? {}
      : { uploadedBefore: dateMillis(input.uploadedBefore) }),
    ...(input.uploadedAfter == null
      ? {}
      : { uploadedAfter: dateMillis(input.uploadedAfter) }),
  };
}

export function normalizeRange(value: unknown): R2Range | undefined {
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
    if (offset < 0)
      throw new RangeError(
        `Invalid range. Starting offset (${offset}) must be greater than or equal to 0.`,
      );
    if (!Number.isInteger(offset)) {
      throw new RangeError(
        `Invalid range. Starting offset (${offset}) must be an integer, not floating point.`,
      );
    }
    out.offset = offset;
  }
  if (input.length != null) {
    const length = Number(input.length);
    if (length < 0)
      throw new RangeError(
        `Invalid range. Length (${length}) must be greater than or equal to 0.`,
      );
    if (!Number.isInteger(length)) {
      throw new RangeError(
        `Invalid range. Length (${length}) must be an integer, not floating point.`,
      );
    }
    if (length === 0)
      throw new Error("get: The requested range is not satisfiable (10039)");
    out.length = length;
  }
  if (input.suffix != null) {
    if (out.offset != null) typeError("Suffix is incompatible with offset.");
    if (out.length != null) typeError("Suffix is incompatible with length.");
    const suffix = Number(input.suffix);
    if (suffix < 0)
      throw new RangeError(
        `Invalid suffix. Suffix (${suffix}) must be greater than or equal to 0.`,
      );
    if (!Number.isInteger(suffix)) {
      throw new RangeError(
        `Invalid range. Suffix (${suffix}) must be an integer, not floating point.`,
      );
    }
    if (suffix === 0)
      throw new Error("get: The requested range is not satisfiable (10039)");
    out.suffix = suffix;
  }
  if (out.offset == null && out.length == null && out.suffix == null) {
    throw new Error(
      "get: We encountered an internal error. Please try again. (10001)",
    );
  }
  return out;
}

export function normalizeHttpMetadata(value: unknown): R2HttpMetadata {
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
  for (const name of [
    "contentType",
    "contentLanguage",
    "contentDisposition",
    "contentEncoding",
    "cacheControl",
  ] as const) {
    if (input[name] != null) out[name] = String(input[name]);
  }
  if (input.cacheExpiry != null)
    out.cacheExpiry = dateMillis(input.cacheExpiry);
  return out;
}

export function normalizeCustomMetadata(
  value: unknown,
): Record<string, string> {
  if (value == null) return {};
  const input = assertObject(value);
  const out: Record<string, string> = {};
  for (const [key, item] of Object.entries(input)) out[key] = String(item);
  return out;
}

export function hexFromBytes(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  );
}

export function bytesFromHex(hex: string): ArrayBuffer {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++)
    bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes.buffer;
}

function parseChecksum(
  value: unknown,
  name: string,
  bytes: number,
  hexChars: number,
): string {
  if (typeof value === "string") {
    if (value.length !== hexChars)
      typeError(`${name} is ${hexChars} hex characters, not ${value.length}`);
    const decoded = value.replace(/[^0-9a-fA-F]/g, "");
    if (decoded.length !== hexChars)
      typeError(`Provided ${name} wasn't a valid hex string`);
    return decoded.toLowerCase();
  }
  let view: Uint8Array | undefined;
  if (value instanceof ArrayBuffer) view = new Uint8Array(value);
  else if (ArrayBuffer.isView(value))
    view = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (!view) typeError("R2_INVALID_OPTIONS");
  if (view.byteLength !== bytes)
    typeError(`${name} is ${bytes} bytes, not ${view.byteLength}`);
  return hexFromBytes(view);
}

export function parseSsecKey(value: unknown): string | undefined {
  if (value == null) return undefined;
  if (typeof value === "string") {
    if (!/^[0-9a-f]+$/.test(value))
      throw new Error("SSE-C Key has invalid format");
    if (value.length !== 64)
      throw new Error("SSE-C Key must be 32 bytes in length");
    return value;
  }
  let view: Uint8Array | undefined;
  if (value instanceof ArrayBuffer) view = new Uint8Array(value);
  else if (ArrayBuffer.isView(value))
    view = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (!view || view.byteLength !== 32)
    throw new Error("SSE-C Key must be 32 bytes in length");
  return hexFromBytes(view);
}

export function parseStorageClass(
  value: unknown,
  operation: "put" | "createMultipartUpload",
): string | undefined {
  if (value == null) return undefined;
  const storageClass = domString(value);
  if (storageClass !== "Standard" && storageClass !== "InfrequentAccess") {
    throw new Error(
      `${operation}: We encountered an internal error. Please try again. (10001)`,
    );
  }
  return storageClass;
}

export function parseChecksumOption(
  input: Record<string, unknown>,
): R2PutWireOptions["checksum"] {
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
    found = {
      algorithm: name,
      hex: parseChecksum(input[name], label, bytes, hex),
    };
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

export function putBody(value: unknown): ReadableStream<unknown> {
  if (value == null) return oneChunk(new Uint8Array());
  if (typeof value === "string") return oneChunk(encoder.encode(value));
  if (value instanceof ArrayBuffer)
    return oneChunk(new Uint8Array(value.slice(0)));
  if (ArrayBuffer.isView(value)) {
    const copy = new Uint8Array(value.byteLength);
    copy.set(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
    return oneChunk(copy);
  }
  if (value instanceof Blob) return value.stream();
  if (value instanceof ReadableStream) return value;
  typeError("R2_INVALID_OPTIONS");
}

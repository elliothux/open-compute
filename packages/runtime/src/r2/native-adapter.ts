// Pinned workerd R2 binding wire; resource identity and permission remain in the authority.
import { isRecord } from "../bindings/private-transport.js";
import type {
  R2Condition,
  R2EtagMatch,
  R2Metadata,
  R2MultipartCreateOptions,
  R2Range,
  R2RawTransport,
} from "./protocol.js";
import { assertKey, normalizeRange } from "./validation.js";

const encoder = new TextEncoder();
const INVALID = "R2_INVALID_OPTIONS";
const MAX_METADATA_BYTES = 1024 * 1024;

function text(value: unknown): string {
  if (typeof value !== "string") throw new TypeError(INVALID);
  return value;
}

function integer(value: unknown): number {
  if (
    typeof value !== "number" &&
    (typeof value !== "string" || !/^\d+$/.test(value))
  )
    throw new TypeError(INVALID);
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < 0) throw new TypeError(INVALID);
  return number;
}

function record(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) throw new TypeError(INVALID);
  return value;
}

function etags(value: unknown): R2EtagMatch[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new TypeError(INVALID);
  return value.map((item: unknown) => {
    const input = record(item);
    switch (input.type) {
      case "wildcard":
        return { kind: "wildcard" };
      case "weak":
        return { kind: "weak", value: text(input.value) };
      case "strong":
        return { kind: "strong", value: text(input.value) };
      default:
        throw new TypeError(INVALID);
    }
  });
}

function condition(value: unknown): R2Condition | undefined {
  if (value === undefined) return undefined;
  const input = record(value);
  if (
    input.secondsGranularity !== undefined &&
    typeof input.secondsGranularity !== "boolean"
  )
    throw new TypeError(INVALID);
  return {
    etagMatches: etags(input.etagMatches),
    etagDoesNotMatch: etags(input.etagDoesNotMatch),
    ...(input.uploadedBefore === undefined
      ? {}
      : { uploadedBefore: integer(input.uploadedBefore) }),
    ...(input.uploadedAfter === undefined
      ? {}
      : { uploadedAfter: integer(input.uploadedAfter) }),
    ...(input.secondsGranularity === undefined
      ? {}
      : { secondsGranularity: input.secondsGranularity }),
  };
}

function pairs(value: unknown): Record<string, string> {
  if (value === undefined) return {};
  if (!Array.isArray(value)) throw new TypeError(INVALID);
  const entries: [string, string][] = value.map((entry: unknown) => {
    const input = record(entry);
    return [text(input.k), text(input.v)];
  });
  if (new Set(entries.map(([key]) => key)).size !== entries.length)
    throw new TypeError(INVALID);
  return Object.fromEntries(entries);
}

function metadataOptions(
  input: Record<string, unknown>,
): R2MultipartCreateOptions {
  const httpFields =
    input.httpFields === undefined ? {} : record(input.httpFields);
  const httpMetadata: Record<string, string | number> = {};
  for (const [key, value] of Object.entries(httpFields)) {
    if (
      ![
        "contentType",
        "contentLanguage",
        "contentDisposition",
        "contentEncoding",
        "cacheControl",
        "cacheExpiry",
      ].includes(key)
    )
      throw new TypeError(INVALID);
    httpMetadata[key] = key === "cacheExpiry" ? integer(value) : text(value);
  }
  return {
    httpMetadata,
    customMetadata: pairs(input.customFields),
    ...(input.storageClass === undefined
      ? {}
      : { storageClass: text(input.storageClass) }),
    ...ssec(input),
  };
}

function ssec(input: Record<string, unknown>) {
  if (input.ssec === undefined) return {};
  const ssecKey = text(record(input.ssec).key);
  if (!/^[0-9a-f]{64}$/.test(ssecKey)) throw new TypeError(INVALID);
  return { ssecKey };
}

function metadata(meta: R2Metadata): Record<string, unknown> {
  const checksums: Record<string, string> = {};
  for (const [index, key] of (
    ["md5", "sha1", "sha256", "sha384", "sha512"] as const
  ).entries()) {
    const checksum = meta.checksums[key];
    if (checksum !== undefined) checksums[index] = checksum;
  }
  return {
    name: meta.key,
    version: meta.version,
    size: String(meta.size),
    etag: meta.etag,
    uploaded: String(meta.uploaded),
    storageClass: meta.storageClass,
    checksums,
    ...(meta.httpMetadata == null
      ? {}
      : {
          httpFields: Object.fromEntries(
            Object.entries(meta.httpMetadata)
              .filter(([, value]) => value != null)
              .map(([key, value]) => [
                key,
                key === "cacheExpiry" ? String(value) : value,
              ]),
          ),
        }),
    ...(meta.customMetadata == null
      ? {}
      : {
          customFields: Object.entries(meta.customMetadata).map(([k, v]) => ({
            k,
            v,
          })),
        }),
    ...(meta.range == null
      ? {}
      : {
          range: Object.fromEntries(
            Object.entries(meta.range)
              .filter(([, value]) => value != null)
              .map(([key, value]) => [key, String(value)]),
          ),
        }),
    ...(meta.ssecKeyMd5 == null
      ? {}
      : { ssec: { algorithm: "AES256", keyMd5: meta.ssecKeyMd5 } }),
  };
}

/** A native PUT begins with bounded JSON, followed by an unbuffered object body. */
async function putInput(request: Request) {
  const size = integer(request.headers.get("cf-r2-metadata-size"));
  if (size < 1 || size > 16384 || !request.body) throw new TypeError(INVALID);
  const reader = request.body.getReader();
  const prefix = new Uint8Array(size);
  let offset = 0;
  let remainder: Uint8Array | undefined;
  try {
    while (offset < size) {
      const next = await reader.read();
      if (next.done) throw new TypeError(INVALID);
      const count = Math.min(size - offset, next.value.byteLength);
      prefix.set(next.value.subarray(0, count), offset);
      offset += count;
      if (count < next.value.byteLength) remainder = next.value.subarray(count);
    }
    // Native R2 encodes lone surrogates as invalid UTF-8; Cloudflare replaces each byte.
    const input: unknown = JSON.parse(new TextDecoder().decode(prefix));
    const parsed = record(input);
    return {
      input: parsed,
      cancel: () => reader.cancel(),
      body: new ReadableStream<Uint8Array>({
        async pull(controller) {
          if (remainder !== undefined) {
            controller.enqueue(remainder);
            remainder = undefined;
            return;
          }
          const next = await reader.read();
          if (next.done) {
            controller.close();
            reader.releaseLock();
          } else controller.enqueue(next.value);
        },
        cancel(reason) {
          return reader.cancel(reason);
        },
      }),
    };
  } catch {
    await reader.cancel().catch(() => {});
    throw new TypeError(INVALID);
  }
}

function reply(
  value: unknown,
  get: boolean,
  body?: ReadableStream<Uint8Array>,
  status = 200,
  errorCode?: number,
): Response {
  const prefix = encoder.encode(JSON.stringify(value));
  if (prefix.byteLength > MAX_METADATA_BYTES)
    throw new Error("R2_INTERNAL_PROTOCOL_ERROR");
  const headers = new Headers({ "content-type": "application/json" });
  if (errorCode !== undefined)
    headers.set(
      "cf-r2-error",
      JSON.stringify({
        version: 1,
        v4code: errorCode,
        message: "R2_PRECONDITION_FAILED",
      }),
    );
  if (!get) return new Response(prefix, { status, headers });
  headers.set("cf-r2-metadata-size", String(prefix.byteLength));
  const reader = body?.getReader();
  let first = true;
  return new Response(
    new ReadableStream<Uint8Array>({
      async pull(controller) {
        if (first) {
          first = false;
          controller.enqueue(prefix);
          return;
        }
        if (!reader) {
          controller.close();
          return;
        }
        const next = await reader.read();
        if (next.done) {
          controller.close();
          reader.releaseLock();
        } else controller.enqueue(next.value);
      },
      cancel(reason) {
        return reader?.cancel(reason);
      },
    }),
    { status, headers },
  );
}

function failure(code: string, status: number, v4code: number): Response {
  return new Response(null, {
    status,
    headers: {
      "cf-r2-error": JSON.stringify({ version: 1, v4code, message: code }),
    },
  });
}

/** Serve native R2 capabilities through the current typed resource transport. */
export async function nativeR2Fetch(
  request: Request,
  bucket: R2RawTransport,
): Promise<Response> {
  let cancel: (() => Promise<void>) | undefined;
  try {
    const get = request.method === "GET";
    if (
      new URL(request.url).hostname !== "fake-host" ||
      (!get && request.method !== "PUT")
    )
      throw new TypeError(INVALID);
    let input: Record<string, unknown>;
    let body: ReadableStream<Uint8Array> | undefined;
    if (get) {
      const raw = request.headers.get("cf-r2-request");
      if (raw === null || encoder.encode(raw).byteLength > 16384)
        throw new TypeError(INVALID);
      input = record(JSON.parse(raw) as unknown);
    } else {
      const decoded = await putInput(request);
      input = decoded.input;
      body = decoded.body;
      cancel = decoded.cancel;
    }
    if (input.version !== 1 || typeof input.method !== "string")
      throw new TypeError(INVALID);
    const method = input.method;
    if (get !== ["head", "get", "list"].includes(method))
      throw new TypeError(INVALID);
    if (
      (method === "put" || method === "createMultipartUpload") &&
      input.storageClass !== undefined &&
      !["Standard", "InfrequentAccess"].includes(text(input.storageClass))
    ) {
      await cancel?.().catch(() => {});
      return failure(
        "We encountered an internal error. Please try again.",
        500,
        10001,
      );
    }
    if (body && method !== "put" && method !== "uploadPart") {
      const reader = body.getReader();
      const next = await reader.read();
      if (!next.done) {
        await reader.cancel();
        throw new TypeError(INVALID);
      }
      reader.releaseLock();
    }
    if (method === "list") {
      const include: string[] = [];
      if (input.include !== undefined) {
        if (!Array.isArray(input.include)) throw new TypeError(INVALID);
        for (const field of input.include) {
          if (field !== 0 && field !== 1) throw new TypeError(INVALID);
          include.push(field === 0 ? "httpMetadata" : "customMetadata");
        }
      }
      const options = {
        prefix: input.prefix === undefined ? "" : text(input.prefix),
        limit:
          input.limit === undefined
            ? 1000
            : Math.min(1000, integer(input.limit)),
        include,
        ...(input.cursor === undefined ? {} : { cursor: text(input.cursor) }),
        ...(input.delimiter === undefined
          ? {}
          : { delimiter: text(input.delimiter) }),
        ...(input.startAfter === undefined
          ? {}
          : { startAfter: text(input.startAfter) }),
      };
      // Native R2 permits fewer than limit entries when response metadata is large.
      // Re-read the same authority cursor with a smaller page; never forge a cursor or drop entries.
      for (;;) {
        const result = await bucket.list(options);
        const value = { ...result, objects: result.objects.map(metadata) };
        if (
          encoder.encode(JSON.stringify(value)).byteLength <= MAX_METADATA_BYTES
        )
          return reply(value, true);
        if (options.limit <= 1) throw new Error("R2_INTERNAL_PROTOCOL_ERROR");
        options.limit = Math.max(1, Math.floor(options.limit / 2));
      }
    }
    if (method === "delete") {
      const keys =
        input.objects === undefined ? [text(input.object)] : input.objects;
      if (!Array.isArray(keys)) throw new TypeError(INVALID);
      await bucket.delete(keys.map((key) => assertKey(text(key))));
      return reply({}, false);
    }
    const key = assertKey(text(input.object));
    switch (method) {
      case "head": {
        const result = await bucket.head(key);
        return result === null
          ? failure("R2_OBJECT_NOT_FOUND", 404, 10007)
          : reply(metadata(result), true);
      }
      case "get": {
        let range: R2Range | undefined;
        if (input.range !== undefined) {
          range = normalizeRange(
            Object.fromEntries(
              Object.entries(record(input.range)).map(([field, value]) => {
                if (!["offset", "length", "suffix"].includes(field))
                  throw new TypeError(INVALID);
                return [field, integer(value)];
              }),
            ),
          );
        }
        if (input.rangeHeader !== undefined) {
          if (range !== undefined) throw new TypeError(INVALID);
          range = normalizeRange(
            new Headers({ range: text(input.rangeHeader) }),
          );
        }
        const onlyIf = condition(input.onlyIf);
        const result = await bucket.get(key, {
          ...(range === undefined ? {} : { range }),
          ...(onlyIf === undefined ? {} : { onlyIf }),
          ...ssec(input),
        });
        if (result === null) return failure("R2_OBJECT_NOT_FOUND", 404, 10007);
        return reply(
          metadata(result.meta),
          true,
          result.body,
          result.body === undefined ? 412 : 200,
          result.body === undefined ? 10031 : undefined,
        );
      }
      case "put": {
        if (!body) throw new TypeError(INVALID);
        let checksum:
          | {
              algorithm: "md5" | "sha1" | "sha256" | "sha384" | "sha512";
              hex: string;
            }
          | undefined;
        for (const algorithm of [
          "md5",
          "sha1",
          "sha256",
          "sha384",
          "sha512",
        ] as const) {
          if (input[algorithm] === undefined) continue;
          if (checksum !== undefined) throw new TypeError(INVALID);
          const value = text(input[algorithm]);
          checksum = {
            algorithm,
            hex:
              algorithm === "md5"
                ? Array.from(atob(value), (char) =>
                    char.charCodeAt(0).toString(16).padStart(2, "0"),
                  ).join("")
                : value,
          };
        }
        const onlyIf = condition(input.onlyIf);
        const result = await bucket.put(key, body, {
          ...metadataOptions(input),
          ...(onlyIf === undefined ? {} : { onlyIf }),
          ...(checksum === undefined ? {} : { checksum }),
        });
        return result === null
          ? failure("R2_PRECONDITION_FAILED", 412, 10031)
          : reply(metadata(result), false);
      }
      case "createMultipartUpload": {
        const result = await bucket.createMultipartUpload(
          key,
          metadataOptions(input),
        );
        return reply({ uploadId: result.uploadId }, false);
      }
      case "uploadPart": {
        if (!body) throw new TypeError(INVALID);
        const result = await bucket.uploadPart(
          key,
          text(input.uploadId),
          integer(input.partNumber),
          body,
          ssec(input).ssecKey,
        );
        return reply({ etag: result.etag }, false);
      }
      case "completeMultipartUpload": {
        if (!Array.isArray(input.parts)) throw new TypeError(INVALID);
        const parts = input.parts.map((item: unknown) => {
          const part = record(item);
          return { partNumber: integer(part.part), etag: text(part.etag) };
        });
        // Cloudflare's complete() result omits HTTP and custom metadata;
        // later reads return the stored values.
        const {
          httpFields: _http,
          customFields: _custom,
          ...completed
        } = metadata(
          await bucket.completeMultipartUpload(
            key,
            text(input.uploadId),
            parts,
          ),
        );
        return reply(completed, false);
      }
      case "abortMultipartUpload":
        await bucket.abortMultipartUpload(key, text(input.uploadId));
        return reply({}, false);
      default:
        throw new TypeError(INVALID);
    }
  } catch (error) {
    await cancel?.().catch(() => {});
    const reported: unknown =
      error instanceof TypeError
        ? error.message
        : error instanceof Error
          ? Object.getOwnPropertyDescriptor(error, "stableCode")?.value
          : undefined;
    const code =
      typeof reported === "string" &&
      /^(?:R2|BINDING)_[A-Z0-9_]{1,127}$/.test(reported)
        ? reported
        : "R2_INTERNAL_PROTOCOL_ERROR";
    return failure(
      code,
      code === "BINDING_PERMISSION_DENIED"
        ? 403
        : error instanceof TypeError || error instanceof SyntaxError
          ? 400
          : 500,
      10001,
    );
  }
}

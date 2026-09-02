import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv, BindingError, ResourceBindingProps } from "../bindings/protocol.js";
import type {
  R2Checksums, R2GetOptions, R2ListOptions, R2ListResult, R2Metadata,
  R2MultipartCreateOptions, R2PutOptions, R2UploadedPart,
} from "./protocol.js";

const CONTENT_TYPE = "application/vnd.open-compute.r2.v1+json";
const FRAME_CONTENT_TYPE = "application/vnd.open-compute.r2.v1+frame";
const MAX_METADATA_BYTES = 16384;

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item: unknown) => typeof item === "string");
}

function optionalString(value: unknown): value is string | null | undefined {
  return value == null || typeof value === "string";
}

function assertChecksums(value: unknown, bindingError: BindingError): asserts value is R2Checksums {
  if (!isRecord(value)) throw bindingError("BINDING_PROTOCOL_ERROR");
  for (const [name, length] of [
    ["md5", 32], ["sha1", 40], ["sha256", 64], ["sha384", 96], ["sha512", 128],
  ] as const) {
    if (value[name] != null && (typeof value[name] !== "string"
        || value[name].length !== length || !/^[0-9a-f]+$/.test(value[name]))) {
      throw bindingError("BINDING_PROTOCOL_ERROR");
    }
  }
}

function assertMetadata(value: unknown, bindingError: BindingError): asserts value is R2Metadata {
  if (!isRecord(value) || typeof value.key !== "string" || typeof value.etag !== "string"
      || typeof value.httpEtag !== "string" || typeof value.size !== "number"
      || !Number.isSafeInteger(value.size) || value.size < 0
      || typeof value.uploaded !== "number" || !Number.isSafeInteger(value.uploaded)
      || typeof value.version !== "string"
      || (value.storageClass !== "Standard" && value.storageClass !== "InfrequentAccess")
      || !optionalString(value.ssecKeyMd5)) {
    throw bindingError("BINDING_PROTOCOL_ERROR");
  }
  assertChecksums(value.checksums, bindingError);
  if (value.httpMetadata != null) {
    if (!isRecord(value.httpMetadata)) throw bindingError("BINDING_PROTOCOL_ERROR");
    for (const [key, item] of Object.entries(value.httpMetadata)) {
      if (!["contentType", "contentLanguage", "contentDisposition", "contentEncoding", "cacheControl", "cacheExpiry"].includes(key)
          || (item != null && (key === "cacheExpiry"
            ? typeof item !== "number" || !Number.isSafeInteger(item) : typeof item !== "string"))) {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
    }
  }
  if (value.customMetadata != null) {
    if (!isRecord(value.customMetadata)
        || Object.values(value.customMetadata).some(item => typeof item !== "string")) {
      throw bindingError("BINDING_PROTOCOL_ERROR");
    }
  }
  if (value.range != null) {
    if (!isRecord(value.range) || Object.entries(value.range).some(([key, item]) =>
      !["offset", "length", "suffix"].includes(key)
      || (item !== null && (typeof item !== "number" || !Number.isSafeInteger(item) || item < 0)))) {
      throw bindingError("BINDING_PROTOCOL_ERROR");
    }
  }
}

function framedBody(header: unknown, stream: ReadableStream<unknown>, bindingError: BindingError): ReadableStream<Uint8Array> {
  if (!(stream instanceof ReadableStream)) throw bindingError("R2_INVALID_OPTIONS");
  const encoded = new TextEncoder().encode(JSON.stringify(header));
  if (encoded.byteLength > MAX_METADATA_BYTES) throw bindingError("R2_METADATA_TOO_LARGE");
  const prefix = new Uint8Array(4 + encoded.byteLength);
  new DataView(prefix.buffer).setUint32(0, encoded.byteLength);
  prefix.set(encoded, 4);
  const reader = stream.getReader();
  let first = true;
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
      if (!(next.value instanceof Uint8Array)) {
        await reader.cancel();
        controller.error(bindingError("R2_INVALID_OPTIONS"));
        return;
      }
      controller.enqueue(next.value);
    },
    cancel(reason) { return reader.cancel(reason); },
  });
}

async function decodeFrame(response: Response, bindingError: BindingError): Promise<{ meta: R2Metadata; body?: ReadableStream<Uint8Array> }> {
  if (!response.body) throw bindingError("BINDING_PROTOCOL_ERROR");
  const reader = response.body.getReader();
  let buffered: Uint8Array = new Uint8Array(0);
  let offset = 0;
  const exact = async (length: number) => {
    const output = new Uint8Array(length);
    let written = 0;
    while (written < length) {
      if (offset === buffered.byteLength) {
        const next = await reader.read();
        if (next.done || !(next.value instanceof Uint8Array)) {
          throw bindingError("BINDING_PROTOCOL_ERROR");
        }
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
  const metadataLength = new DataView((await exact(4)).buffer).getUint32(0);
  if (metadataLength > MAX_METADATA_BYTES) throw bindingError("BINDING_PROTOCOL_ERROR");
  const frame: unknown = JSON.parse(new TextDecoder().decode(await exact(metadataLength)));
  if (!isRecord(frame) || !frame.meta) throw bindingError("BINDING_PROTOCOL_ERROR");
  assertMetadata(frame.meta, bindingError);
  if (frame.hasBody !== true) {
    const terminal = await reader.read();
    if (!terminal.done) throw bindingError("BINDING_PROTOCOL_ERROR");
    return { meta: frame.meta };
  }
  const body = new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (offset < buffered.byteLength) {
        controller.enqueue(buffered.subarray(offset));
        offset = buffered.byteLength;
        return;
      }
      const next = await reader.read();
      if (next.done) controller.close();
      else if (next.value instanceof Uint8Array) controller.enqueue(next.value);
      else controller.error(bindingError("BINDING_PROTOCOL_ERROR"));
    },
    cancel(reason) { return reader.cancel(reason); },
  });
  return { meta: frame.meta, body };
}

export function makeR2TransportBase(bindingError: BindingError, currentStartupGeneration: () => string, tokenHeader: string) {
  return class extends WorkerEntrypoint<BindingEnv, ResourceBindingProps> {
    #props() {
      const props = this.ctx.props;
      if (!props
        || typeof props.bindingId !== "string"
        || typeof props.versionId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || !Number.isSafeInteger(props.resourceSpecGeneration)
        || props.resourceSpecGeneration < 1) {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
      return props;
    }

    async #request(operation: string, body: BodyInit, permission: "read" | "write", contentType = CONTENT_TYPE) {
      const props = this.#props();
      if (!props.permissions[permission]) throw bindingError("BINDING_PERMISSION_DENIED");
      const response = await this.env.BINDING_BACKEND.fetch(
        `http://binding-backend/internal/bindings/v1/r2/${props.bindingId}/${operation}`,
        {
          method: "POST",
          headers: {
            "content-type": contentType,
            [tokenHeader]: this.env.BINDING_BACKEND_TOKEN,
            "x-open-compute-startup-generation": currentStartupGeneration(),
            "x-open-compute-version-id": props.versionId,
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

    async head(key: string): Promise<R2Metadata | null> {
      const response = await this.#request("head", JSON.stringify({ key }), "read");
      if (response.status === 204) return null;
      const meta: unknown = await response.json();
      assertMetadata(meta, bindingError);
      return meta;
    }

    async get(key: string, options: R2GetOptions) {
      const response = await this.#request("get", JSON.stringify({ key, options }), "read", FRAME_CONTENT_TYPE);
      return response.status === 204 ? null : decodeFrame(response, bindingError);
    }

    async put(key: string, body: ReadableStream<unknown>, options: R2PutOptions): Promise<R2Metadata | null> {
      const response = await this.#request("put", framedBody({ key, options }, body, bindingError), "write", FRAME_CONTENT_TYPE);
      if (response.status === 204) return null;
      const meta: unknown = await response.json();
      assertMetadata(meta, bindingError);
      return meta;
    }

    async delete(keys: string[]) {
      await this.#request("delete", JSON.stringify({ keys }), "write");
    }

    async list(options: R2ListOptions): Promise<R2ListResult> {
      const response = await this.#request("list", JSON.stringify(options), "read");
      const result: unknown = await response.json();
      if (!isRecord(result) || !Array.isArray(result.objects) || typeof result.truncated !== "boolean"
          || (result.truncated ? typeof result.cursor !== "string" : result.cursor != null)
          || !stringArray(result.delimitedPrefixes)) throw bindingError("BINDING_PROTOCOL_ERROR");
      const objects: R2Metadata[] = [];
      for (const object of result.objects) {
        assertMetadata(object, bindingError);
        objects.push(object);
      }
      if (result.truncated) {
        if (typeof result.cursor !== "string") throw bindingError("BINDING_PROTOCOL_ERROR");
        return { objects, truncated: true, cursor: result.cursor, delimitedPrefixes: result.delimitedPrefixes };
      }
      return { objects, truncated: false, delimitedPrefixes: result.delimitedPrefixes };
    }

    async createMultipartUpload(key: string, options: R2MultipartCreateOptions) {
      const response = await this.#request("createMultipartUpload", JSON.stringify({ key, options }), "write");
      const result: unknown = await response.json();
      if (!isRecord(result) || typeof result.key !== "string" || typeof result.uploadId !== "string") {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
      return { key: result.key, uploadId: result.uploadId };
    }

    async uploadPart(key: string, uploadId: string, partNumber: number, body: ReadableStream<unknown>, ssecKey?: string) {
      const response = await this.#request(
        "uploadPart",
        framedBody({ key, uploadId, partNumber, ssecKey }, body, bindingError),
        "write",
        FRAME_CONTENT_TYPE,
      );
      const result: unknown = await response.json();
      if (!isRecord(result) || typeof result.partNumber !== "number" || typeof result.etag !== "string") {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
      return { partNumber: result.partNumber, etag: result.etag } satisfies R2UploadedPart;
    }

    async completeMultipartUpload(key: string, uploadId: string, parts: R2UploadedPart[]) {
      const response = await this.#request("completeMultipartUpload", JSON.stringify({ key, uploadId, parts }), "write");
      const meta: unknown = await response.json();
      assertMetadata(meta, bindingError);
      return meta;
    }

    async abortMultipartUpload(key: string, uploadId: string) {
      await this.#request("abortMultipartUpload", JSON.stringify({ key, uploadId }), "write");
    }

    async fetch(): Promise<never> {
      throw bindingError("BINDING_PERMISSION_DENIED");
    }
  };
}

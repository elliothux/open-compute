import { WorkerEntrypoint } from "cloudflare:workers";
import { bytes, modulesFor } from "./loaded-isolate-modules.js";
export { modulesFor } from "./loaded-isolate-modules.js";
import { handleWorkflow } from "./workflow-host.js";
import { tenantEnv } from "./loaded-isolate-bindings.js";
export { tenantEnv } from "./loaded-isolate-bindings.js";
export { WorkflowBindingTransportV2 } from "./workflow-binding-v2.js";
export { WorkflowBindingTransport } from "./workflow-host.js";
import { makeR2TransportBase } from "./r2-transport.js";
import { makeD1TransportBase } from "./d1-transport.js";

const SOURCE_PATH = "/internal/runtime/v1/deployments/resolve";
const TOKEN_HEADER = "x-open-compute-internal-token";
const BINDING_TOKEN_HEADER = "x-open-compute-binding-token";
const BINDING_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+json";
const BINDING_FRAME_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+frame";
const MAX_BINDING_KEY_BYTES = 512;
const MAX_KV_VALUE_BYTES = 25 * 1024 * 1024;
const MAX_KV_KEYS = 100;
const MAX_QUEUE_MESSAGES = 100;
const MAX_QUEUE_BODY_BYTES = 128 * 1024;
const MAX_QUEUE_BATCH_BYTES = 256 * 1024;
let startupGeneration;
const assembling = new Map();
const seenHashes = new Map();
const INTERNAL_HEADERS = [
  TOKEN_HEADER,
  "x-open-compute-account-id",
  "x-open-compute-worker-id",
  "x-open-compute-deployment-id",
  "x-open-compute-loader-key",
  "x-open-compute-worker-code-sha256",
  "x-open-compute-entrypoint",
  "x-open-compute-original-method",
  "x-open-compute-original-url",
  "x-open-compute-route-generation",
  "x-open-compute-request-id",
  "x-open-compute-binding-id",
  "x-open-compute-binding-token",
  "x-open-compute-descriptor-sha256",
  "x-open-compute-namespace-resource-id",
  "x-open-compute-object-id",
  "x-open-compute-object-generation",
  "x-open-compute-class-name",
  "x-open-compute-do-method",
  "x-open-compute-do-url",
  "x-open-compute-do-operation",
  "x-open-compute-startup-generation",
  "forwarded",
  "x-forwarded-for",
  "x-forwarded-host",
  "x-forwarded-proto",
];

export const PROFILE = Object.freeze({ cpuMs: 50, subRequests: 16 });

function policyInteger(env, name, maximum) {
  const value = Number(env[name]);
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return value;
}

export function doPolicy(env) {
  return Object.freeze({
    maxObjectNameBytes: policyInteger(env, "DO_MAX_OBJECT_NAME_BYTES", 1024),
    maxRpcRequestBytes: policyInteger(env, "DO_MAX_RPC_REQUEST_BYTES", 16 * 1024 * 1024),
    maxRpcResponseBytes: policyInteger(env, "DO_MAX_RPC_RESPONSE_BYTES", 16 * 1024 * 1024),
    maxFetchBodyBytes: policyInteger(env, "DO_MAX_FETCH_BODY_BYTES", 64 * 1024 * 1024),
    dispatchTimeoutMs: policyInteger(env, "DO_DISPATCH_TIMEOUT_MS", 5 * 60 * 1000),
    maxInFlightDispatches: policyInteger(env, "DO_MAX_IN_FLIGHT_DISPATCHES", 4096),
  });
}

export function currentStartupGeneration(seed) {
  if (!startupGeneration) startupGeneration = seed || crypto.randomUUID();
  if (seed && startupGeneration !== seed) throw bindingError("BINDING_PROTOCOL_ERROR");
  return startupGeneration;
}

function stableError(code, status, requestId) {
  return Response.json({
    ok: false,
    error: { code, message: "worker request failed", requestId: requestId || null },
  }, { status });
}

function classify(error) {
  const message = String(error && error.message ? error.message : error);
  if (/entrypoint|no such entrypoint|was not found/i.test(message)) {
    return ["ENTRYPOINT_NOT_FOUND", 404];
  }
  if (/limit|cpu time|subrequest/i.test(message)) {
    return ["RESOURCE_LIMIT_EXCEEDED", 429];
  }
  if (/syntax|parse|unexpected|module|wasm|initializ|startup/i.test(message)) {
    return ["BUNDLE_RUNTIME_INVALID", 422];
  }
  return ["RUNTIME_INTERNAL", 500];
}

function assertEnvelope(request, validation, entrypointName) {
  const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
  const expected = request.headers.get("x-open-compute-worker-code-sha256") || "";
  const parts = loaderKey.split("/");
  if (parts.length !== 3 || parts.some((part) => !/^[0-9a-f]{8}-[0-9a-f-]{27}$/.test(part))) {
    throw new Error("invalid loader key");
  }
  if (!/^[0-9a-f]{64}$/.test(expected)) throw new Error("invalid descriptor hash");
  if (entrypointName && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)) {
    throw new Error("invalid entrypoint");
  }
  const routeGeneration = Number(request.headers.get("x-open-compute-route-generation"));
  if (!Number.isSafeInteger(routeGeneration)
      || (validation ? routeGeneration < 0 : routeGeneration < 1)) {
    throw new Error("invalid route generation");
  }
  return {
    loaderKey,
    expected,
    routeGeneration,
    runtimeKey: `${validation ? "validate" : "runtime"}/${loaderKey}/${expected}/g/${routeGeneration}/${entrypointName || "default"}`,
  };
}

export async function resolveSnapshot(env, envelope, validation, probe, internalToken) {
  const response = await env.RUNTIME_SOURCE.fetch(`http://runtime-source${SOURCE_PATH}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      [TOKEN_HEADER]: internalToken,
    },
    body: JSON.stringify({
      startupGeneration: currentStartupGeneration(internalToken),
      key: envelope.loaderKey,
      expectedWorkerCodeSha256: envelope.expected,
      scope: validation ? (probe ? "probe" : "validation") : "runtime",
    }),
  });
  if (!response.ok) {
    const code = response.headers.get("x-open-compute-error-code") || "RUNTIME_INTERNAL";
    const error = new Error(code);
    error.stableCode = code;
    throw error;
  }
  const snapshot = await response.json();
  if (snapshot.loaderKey !== envelope.loaderKey || snapshot.workerCodeSha256 !== envelope.expected) {
    const error = new Error("DEPLOYMENT_INVARIANT_VIOLATION");
    error.stableCode = "DEPLOYMENT_INVARIANT_VIOLATION";
    throw error;
  }
  return snapshot;
}

function assembleOnce(key, build) {
  const current = assembling.get(key);
  if (current) return current;
  const pending = build().finally(() => {
    if (assembling.get(key) === pending) assembling.delete(key);
  });
  assembling.set(key, pending);
  return pending;
}

export function bindingError(code) {
  const error = new Error(code);
  error.name = "Error";
  error.stableCode = code;
  error.stack = `Error: ${code}`;
  return error;
}

function assertKey(key) {
  if (typeof key !== "string" || !key || key === "." || key === "..") {
    throw new TypeError("KV_KEY_INVALID");
  }
  for (let i = 0; i < key.length; i++) {
    const code = key.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = key.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) throw new TypeError("KV_KEY_INVALID");
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      throw new TypeError("KV_KEY_INVALID");
    }
  }
  if (new TextEncoder().encode(key).byteLength > MAX_BINDING_KEY_BYTES) {
    throw new TypeError("KV_KEY_TOO_LARGE");
  }
}

function assertPrefix(prefix) {
  if (typeof prefix !== "string") throw new TypeError("KV_INVALID_OPTIONS");
  for (let i = 0; i < prefix.length; i++) {
    const code = prefix.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = prefix.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) throw new TypeError("KV_KEY_INVALID");
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      throw new TypeError("KV_KEY_INVALID");
    }
  }
  if (new TextEncoder().encode(prefix).byteLength > MAX_BINDING_KEY_BYTES) {
    throw new TypeError("KV_KEY_TOO_LARGE");
  }
}

function assertSafeSeconds(value, minimum) {
  if (!Number.isSafeInteger(value) || value < minimum) throw new TypeError("KV_INVALID_OPTIONS");
}

function getOptions(input, many) {
  let type = "text";
  let cacheTtl;
  if (input !== undefined) {
    if (typeof input === "string") type = input;
    else if (input && typeof input === "object" && !Array.isArray(input)) {
      const keys = Object.keys(input);
      if (keys.some((key) => key !== "type" && key !== "cacheTtl")) {
        throw new TypeError("KV_INVALID_OPTIONS");
      }
      if (input.type !== undefined) type = input.type;
      if (input.cacheTtl !== undefined) {
        assertSafeSeconds(input.cacheTtl, 30);
        cacheTtl = input.cacheTtl;
      }
    } else throw new TypeError("KV_INVALID_OPTIONS");
  }
  const supported = many ? ["text", "json"] : ["text", "json", "arrayBuffer", "stream"];
  if (!supported.includes(type)) throw new TypeError("KV_INVALID_OPTIONS");
  return { type, cacheTtl };
}

function assertMetadata(value, seen = new WeakSet()) {
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
    for (const key of Object.keys(value)) assertMetadata(value[key], seen);
  }
  seen.delete(value);
}

function putOptions(input) {
  if (input === undefined) return { metadataPresent: false };
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    throw new TypeError("KV_INVALID_OPTIONS");
  }
  const keys = Object.keys(input);
  if (keys.some((key) => !["expiration", "expirationTtl", "metadata"].includes(key))) {
    throw new TypeError("KV_INVALID_OPTIONS");
  }
  if (input.expiration !== undefined && input.expirationTtl !== undefined) {
    throw new TypeError("KV_INVALID_OPTIONS");
  }
  if (input.expiration !== undefined) assertSafeSeconds(input.expiration, 1);
  if (input.expirationTtl !== undefined) assertSafeSeconds(input.expirationTtl, 60);
  const metadataPresent = Object.prototype.hasOwnProperty.call(input, "metadata")
    && input.metadata !== undefined;
  if (metadataPresent) assertMetadata(input.metadata);
  return {
    expiration: input.expiration,
    expirationTtl: input.expirationTtl,
    metadata: metadataPresent ? input.metadata : undefined,
    metadataPresent,
  };
}

function valueStream(value) {
  if (typeof value === "string") {
    const bytes = new TextEncoder().encode(value);
    return { stream: new Blob([bytes]).stream(), knownLength: bytes.byteLength };
  }
  if (value instanceof ArrayBuffer) {
    const bytes = new Uint8Array(value);
    return { stream: new Blob([bytes]).stream(), knownLength: bytes.byteLength };
  }
  if (ArrayBuffer.isView(value)) {
    const bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    return { stream: new Blob([bytes]).stream(), knownLength: bytes.byteLength };
  }
  if (value instanceof ReadableStream) return { stream: value, knownLength: undefined };
  throw new TypeError("KV value must be a string, buffer, view, or ReadableStream");
}

function framedPutBody(header, value) {
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
  return new ReadableStream({
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
        controller.error(new TypeError("KV stream chunks must be bytes"));
        return;
      }
      total += next.value.byteLength;
      if (total > MAX_KV_VALUE_BYTES) {
        const prior = total - next.value.byteLength;
        const firstOverflowByte = next.value.subarray(0, MAX_KV_VALUE_BYTES - prior + 1);
        controller.enqueue(firstOverflowByte);
        await reader.cancel();
        controller.close();
        return;
      }
      controller.enqueue(next.value);
    },
    cancel(reason) { return reader.cancel(reason); },
  });
}

function decodeEntry(view, state) {
  if (state.offset + 17 > view.byteLength) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
  const found = view.getUint8(state.offset++);
  const expiration = view.getBigInt64(state.offset);
  state.offset += 8;
  const metadataLength = view.getUint32(state.offset);
  state.offset += 4;
  let metadata = null;
  if (metadataLength !== 0xffffffff) {
    if (state.offset + metadataLength > view.byteLength) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    metadata = JSON.parse(new TextDecoder().decode(
      new Uint8Array(view.buffer, view.byteOffset + state.offset, metadataLength),
    ));
    state.offset += metadataLength;
  }
  const valueLength = view.getUint32(state.offset);
  state.offset += 4;
  if (!found) {
    if (valueLength !== 0xffffffff) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    return { value: null, metadata: null, expiration: null };
  }
  if (valueLength === 0xffffffff || state.offset + valueLength > view.byteLength) {
    throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
  }
  const value = new Uint8Array(valueLength);
  value.set(new Uint8Array(view.buffer, view.byteOffset + state.offset, valueLength));
  state.offset += valueLength;
  return { value, metadata, expiration: expiration < 0n ? null : Number(expiration) };
}

function decodeValue(bytes, type) {
  if (bytes === null) return null;
  if (type === "arrayBuffer") return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const text = new TextDecoder().decode(bytes);
  if (type === "json") return JSON.parse(text);
  return text;
}

async function decodeStreamValue(stream, type) {
  if (stream === null || type === "stream") return stream;
  const response = new Response(stream);
  if (type === "arrayBuffer") return response.arrayBuffer();
  const text = await response.text();
  if (type === "json") return JSON.parse(text);
  return text;
}

async function decodeSingleEntry(response) {
  if (!response.body) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
  const reader = response.body.getReader();
  let buffered = new Uint8Array(0);
  let offset = 0;
  const exact = async (length) => {
    const output = new Uint8Array(length);
    let written = 0;
    while (written < length) {
      if (offset === buffered.byteLength) {
        const next = await reader.read();
        if (next.done || !(next.value instanceof Uint8Array)) {
          throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
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
  const prefix = await exact(17);
  if (new TextDecoder().decode(prefix.subarray(0, 4)) !== "KVS1") {
    throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
  }
  const view = new DataView(prefix.buffer, prefix.byteOffset, prefix.byteLength);
  const found = view.getUint8(4);
  const expiration = view.getBigInt64(5);
  const metadataLength = view.getUint32(13);
  let metadata = null;
  if (metadataLength !== 0xffffffff) {
    metadata = JSON.parse(new TextDecoder().decode(await exact(metadataLength)));
  }
  const valueLength = new DataView((await exact(4)).buffer).getUint32(0);
  if (!found) {
    if (valueLength !== 0xffffffff || metadataLength !== 0xffffffff) {
      throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    }
    const terminal = await reader.read();
    if (!terminal.done) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    return { value: null, metadata: null, expiration: null };
  }
  if (valueLength === 0xffffffff) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
  let remaining = valueLength;
  const value = new ReadableStream({
    async pull(controller) {
      if (remaining === 0) {
        const terminal = await reader.read();
        if (!terminal.done) {
          controller.error(bindingError("KV_INTERNAL_PROTOCOL_ERROR"));
          return;
        }
        controller.close();
        return;
      }
      if (offset < buffered.byteLength) {
        const count = Math.min(remaining, buffered.byteLength - offset);
        controller.enqueue(buffered.subarray(offset, offset + count));
        offset += count;
        remaining -= count;
        return;
      }
      const next = await reader.read();
      if (next.done || !(next.value instanceof Uint8Array) || next.value.byteLength > remaining) {
        controller.error(bindingError("KV_INTERNAL_PROTOCOL_ERROR"));
        return;
      }
      remaining -= next.value.byteLength;
      controller.enqueue(next.value);
    },
    cancel(reason) {
      remaining = 0;
      return reader.cancel(reason);
    },
  });
  return { value, metadata, expiration: expiration < 0n ? null : Number(expiration) };
}

export class KVNamespace extends WorkerEntrypoint {
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

  async #request(operation, body, permission, contentType = BINDING_CONTENT_TYPE) {
    const props = this.#props();
    if (!props.permissions[permission]) {
      throw bindingError("BINDING_PERMISSION_DENIED");
    }
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/kv/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": contentType,
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

  async #entries(operation, keys, options) {
    const response = await this.#request(
      operation,
      JSON.stringify({ keys, cacheTtl: options.cacheTtl }),
      "read",
      BINDING_FRAME_CONTENT_TYPE,
    );
    if (operation === "get" || operation === "get-with-metadata") {
      return [await decodeSingleEntry(response)];
    }
    const buffer = await response.arrayBuffer();
    const view = new DataView(buffer);
    const magic = new TextDecoder().decode(new Uint8Array(buffer, 0, 4));
    const state = { offset: 4 };
    if (magic !== "KVB1" || buffer.byteLength < 6) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    const count = view.getUint16(4);
    state.offset = 6;
    if (count !== keys.length) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    const entries = [];
    for (let i = 0; i < count; i++) entries.push(decodeEntry(view, state));
    if (state.offset !== buffer.byteLength) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
    return entries;
  }

  async get(keyOrKeys, typeOrOptions) {
    const many = Array.isArray(keyOrKeys);
    const keys = many ? keyOrKeys : [keyOrKeys];
    if (many && keys.length > MAX_KV_KEYS) throw new TypeError("KV_TOO_MANY_KEYS");
    for (const key of keys) assertKey(key);
    const options = getOptions(typeOrOptions, many);
    const entries = await this.#entries(many ? "get-many" : "get", keys, options);
    if (!many) return decodeStreamValue(entries[0].value, options.type);
    const result = new Map();
    for (let i = 0; i < keys.length; i++) {
      if (!result.has(keys[i])) result.set(keys[i], decodeValue(entries[i].value, options.type));
    }
    return result;
  }

  async getWithMetadata(keyOrKeys, typeOrOptions) {
    const many = Array.isArray(keyOrKeys);
    const keys = many ? keyOrKeys : [keyOrKeys];
    if (many && keys.length > MAX_KV_KEYS) throw new TypeError("KV_TOO_MANY_KEYS");
    for (const key of keys) assertKey(key);
    const options = getOptions(typeOrOptions, many);
    const entries = await this.#entries(many ? "get-many" : "get-with-metadata", keys, options);
    if (!many) {
      return { value: await decodeStreamValue(entries[0].value, options.type), metadata: entries[0].metadata };
    }
    const convert = (entry) => ({ value: decodeValue(entry.value, options.type), metadata: entry.metadata });
    const result = new Map();
    for (let i = 0; i < keys.length; i++) {
      if (!result.has(keys[i])) result.set(keys[i], convert(entries[i]));
    }
    return result;
  }

  async put(key, value, options) {
    assertKey(key);
    const normalized = putOptions(options);
    const header = {
      key,
      expiration: normalized.expiration,
      expirationTtl: normalized.expirationTtl,
      metadata: normalized.metadata,
      metadataPresent: normalized.metadataPresent,
    };
    await this.#request(
      "put",
      framedPutBody(header, value),
      "write",
      BINDING_FRAME_CONTENT_TYPE,
    );
  }

  async delete(key) {
    assertKey(key);
    await this.#request(
      "delete",
      JSON.stringify({ key }),
      "write",
      BINDING_FRAME_CONTENT_TYPE,
    );
  }

  async list(options = {}) {
    if (!options || typeof options !== "object" || Array.isArray(options)) {
      throw new TypeError("KV_INVALID_OPTIONS");
    }
    if (Object.keys(options).some((key) => !["prefix", "limit", "cursor"].includes(key))) {
      throw new TypeError("KV_INVALID_OPTIONS");
    }
    const prefix = options.prefix === undefined ? "" : options.prefix;
    assertPrefix(prefix);
    const limit = options.limit === undefined ? 1000 : options.limit;
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 1000) {
      throw new TypeError("KV_INVALID_OPTIONS");
    }
    if (options.cursor !== undefined && typeof options.cursor !== "string") {
      throw new TypeError("KV_INVALID_OPTIONS");
    }
    const response = await this.#request(
      "list",
      JSON.stringify({ prefix, limit, cursor: options.cursor }),
      "read",
      BINDING_FRAME_CONTENT_TYPE,
    );
    const result = await response.json();
    for (const key of result.keys) {
      if (key.expiration === null) delete key.expiration;
      if (key.metadata === null) delete key.metadata;
    }
    if (result.cursor === null) delete result.cursor;
    return result;
  }

  async echoStream(stream) {
    if (!(stream instanceof ReadableStream)) {
      throw new TypeError("binding stream must be a byte ReadableStream");
    }
    const response = await this.#request(
      "echo",
      stream,
      "read",
      "application/vnd.open-compute.kv.v1+octet-stream",
    );
    return response.body;
  }

  async fetch() {
    throw bindingError("BINDING_PERMISSION_DENIED");
  }
}

const R2TransportBase = makeR2TransportBase(
  bindingError,
  currentStartupGeneration,
  BINDING_TOKEN_HEADER,
);

export class R2Transport extends R2TransportBase {}

const D1TransportBase = makeD1TransportBase(
  bindingError,
  currentStartupGeneration,
  BINDING_TOKEN_HEADER,
);

export class D1Transport extends D1TransportBase {}

export class QueueTransport extends WorkerEntrypoint {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.bindingId !== "string"
        || typeof props.deploymentId !== "string" || typeof props.queueId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || !Number.isSafeInteger(props.queueLifecycleGeneration)
        || props.queueLifecycleGeneration < 1) {
      throw bindingError("QUEUE_INVARIANT_VIOLATION");
    }
    return props;
  }

  async #request(operation, body) {
    const props = this.#props();
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/queue/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": body === undefined
            ? "application/json"
            : "application/vnd.open-compute.queue.v1+frame",
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
      const code = response.headers.get("x-open-compute-error-code")
        || "QUEUE_STORAGE_UNAVAILABLE";
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError(code);
    }
    const result = await response.json();
    if (!result || typeof result !== "object") throw bindingError("QUEUE_INVARIANT_VIOLATION");
    return result;
  }

  send(frame) {
    return this.#request("send", frame);
  }

  sendBatch(frame) {
    return this.#request("batch", frame);
  }

  metrics() {
    return this.#request("metrics");
  }
}

export class DoTransport extends WorkerEntrypoint {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.accountId !== "string" || typeof props.workerId !== "string"
        || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string"
        || typeof props.namespaceResourceId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || !Number.isSafeInteger(props.routeGeneration) || props.routeGeneration < 1) {
      throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    }
    return props;
  }

  #headers(objectId) {
    const props = this.#props();
    if (typeof objectId !== "string" || !/^[0-9a-f]{64}$/.test(objectId)) {
      throw bindingError("DO_ID_INVALID");
    }
    return {
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-account-id": props.accountId,
      "x-open-compute-worker-id": props.workerId,
      "x-open-compute-binding-id": props.bindingId,
      "x-open-compute-deployment-id": props.deploymentId,
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-route-generation": String(props.routeGeneration),
      "x-open-compute-namespace-resource-id": props.namespaceResourceId,
      "x-open-compute-object-id": objectId,
      "x-open-compute-request-id": crypto.randomUUID(),
    };
  }

  async fetch(request) {
    const objectId = new URL(request.url).pathname.slice(1);
    const headers = new Headers(request.headers);
    const tenantMethod = headers.get("x-open-compute-do-method") || request.method;
    const tenantUrl = headers.get("x-open-compute-do-url") || "https://do.invalid/";
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    for (const [name, value] of Object.entries(this.#headers(objectId))) headers.set(name, value);
    headers.set("x-open-compute-do-method", tenantMethod);
    headers.set("x-open-compute-do-url", tenantUrl);
    headers.set("x-open-compute-do-operation", "fetch");
    const init = { method: request.method, headers, body: request.body, redirect: "manual" };
    if (request.method === "GET" || request.method === "HEAD") delete init.body;
    return this.env.DO_ROUTER.fetch(new Request(
      "http://do-router/internal/do/v1/fetch",
      init,
    ));
  }

  async dispatchRpc(objectId, method, args) {
    const response = await this.env.DO_ROUTER.fetch(
      "http://do-router/internal/do/v1/rpc",
      {
        method: "POST",
        headers: {
          ...this.#headers(objectId),
          "content-type": "application/json",
          "x-open-compute-do-operation": "rpc",
        },
        body: JSON.stringify({ method, args }),
      },
    );
    if (!response.ok) {
      throw bindingError(response.headers.get("x-open-compute-error-code") || "DO_RUNTIME_EXCEPTION");
    }
    const payload = await response.json();
    return payload.value;
  }
}

export class AlarmIndex extends WorkerEntrypoint {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.namespaceResourceId !== "string"
        || typeof props.objectId !== "string" || !/^[0-9a-f]{64}$/.test(props.objectId)
        || !Number.isSafeInteger(props.objectGeneration) || props.objectGeneration < 1) {
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    }
    return props;
  }

  async #request(operation, mutation = {}) {
    const props = this.#props();
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/alarms/v1/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-request-id": crypto.randomUUID(),
        },
        body: JSON.stringify({
          namespaceResourceId: props.namespaceResourceId,
          objectId: props.objectId,
          objectGeneration: props.objectGeneration,
          ...mutation,
        }),
      },
    );
    if (!response.ok) {
      throw bindingError(response.headers.get("x-open-compute-error-code")
        || "DO_ALARM_INDEX_UNAVAILABLE");
    }
  }

  async upsert(row) {
    if (!row || !Number.isSafeInteger(row.scheduledTimeMs) || row.scheduledTimeMs <= 0
        || !Number.isSafeInteger(row.retryCount) || row.retryCount < 0 || row.retryCount > 6
        || typeof row.rowToken !== "string") {
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    }
    await this.#request("upsert", row);
  }

  async delete(rowToken) {
    if (typeof rowToken !== "string") throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    await this.#request("delete", { rowToken });
  }

  async clear() {
    await this.#request("clear");
  }
}

function tenantRequest(request) {
  const headers = new Headers(request.headers);
  const method = request.headers.get("x-open-compute-original-method") || "GET";
  const url = request.headers.get("x-open-compute-original-url") || "https://worker.invalid/";
  for (const name of INTERNAL_HEADERS) headers.delete(name);
  const init = { method, headers, body: request.body, redirect: "manual" };
  if (method === "GET" || method === "HEAD") delete init.body;
  return new Request(url, init);
}

export class OutboundGateway extends WorkerEntrypoint {
  async fetch(request) {
    const url = new URL(request.url);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      throw new TypeError("OUTBOUND_DENIED");
    }
    return fetch(new Request(request, { redirect: "follow" }));
  }
}

async function handle(request, env, ctx, validation) {
  const requestId = request.headers.get("x-open-compute-request-id") || crypto.randomUUID();
  try {
    const entrypoint = request.headers.get("x-open-compute-entrypoint") || undefined;
    const envelope = assertEnvelope(request, validation, entrypoint);
    const internalToken = request.headers.get(TOKEN_HEADER) || "";
    // Resolve and verify on every path, including a warm WorkerLoader key.
    const snapshot = await resolveSnapshot(env, envelope, validation, Boolean(entrypoint), internalToken);
    const prior = seenHashes.get(envelope.runtimeKey);
    if (prior && prior !== snapshot.workerCodeSha256) {
      const error = new Error("DEPLOYMENT_INVARIANT_VIOLATION");
      error.stableCode = "DEPLOYMENT_INVARIANT_VIOLATION";
      throw error;
    }
    seenHashes.set(envelope.runtimeKey, snapshot.workerCodeSha256);
    const code = await assembleOnce(envelope.runtimeKey, async () => {
      const built = modulesFor(snapshot, validation, entrypoint);
      const deploymentId = envelope.loaderKey.split("/")[2];
      return {
        compatibilityDate: snapshot.compatibilityDate,
        compatibilityFlags: snapshot.compatibilityFlags,
        mainModule: built.mainModule,
        modules: built.modules,
        env: validation ? {} : tenantEnv(snapshot, ctx, deploymentId, doPolicy(env)),
        globalOutbound: validation ? null : ctx.exports.OutboundGateway({
          props: { deploymentId, policyVersion: 1 },
        }),
        limits: PROFILE,
      };
    });
    let cold = false;
    const stub = env.LOADER.get(envelope.runtimeKey, async () => {
      cold = true;
      return code;
    });
    const target = stub.getEntrypoint(validation ? undefined : entrypoint, { limits: PROFILE });
    const response = await target.fetch(validation ? "https://validation.invalid/" : tenantRequest(request));
    if (validation) {
      const body = await response.text();
      if (response.status !== 200 || body !== "open-compute-validation-v1") {
        throw new Error("validation nonce mismatch");
      }
      return new Response(null, { status: 204 });
    }
    const headers = new Headers(response.headers);
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    headers.set("x-open-compute-request-id", requestId);
    headers.set("x-open-compute-loader-outcome", cold ? "cold" : "warm");
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  } catch (error) {
    const stable = error && error.stableCode;
    if (stable) {
      const status = stable === "DEPLOYMENT_NOT_READY" ? 409
        : stable === "ARTIFACT_UNAVAILABLE" ? 503
        : stable === "BUNDLE_RUNTIME_INVALID" ? 422
        : 500;
      return stableError(stable, status, requestId);
    }
    const [code, status] = classify(error);
    return stableError(code, status, requestId);
  }
}

function customEventMessageBody(message) {
  if (!message || typeof message !== "object"
      || typeof message.bodyBase64 !== "string") {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  const raw = bytes(message.bodyBase64);
  if (raw.byteLength > MAX_QUEUE_BODY_BYTES) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  let body;
  switch (message.contentType) {
    case "json":
      body = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw));
      break;
    case "text":
      body = new TextDecoder("utf-8", { fatal: true }).decode(raw);
      break;
    case "bytes":
      body = raw;
      break;
    default:
      throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  return { body, byteLength: raw.byteLength };
}

async function customEventTarget(request, env, ctx) {
  const entrypoint = request.headers.get("x-open-compute-entrypoint") || undefined;
  const envelope = assertEnvelope(request, false, entrypoint);
  const internalToken = request.headers.get(TOKEN_HEADER) || "";
  const snapshot = await resolveSnapshot(env, envelope, false, Boolean(entrypoint), internalToken);
  const prior = seenHashes.get(envelope.runtimeKey);
  if (prior && prior !== snapshot.workerCodeSha256) {
    throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
  }
  seenHashes.set(envelope.runtimeKey, snapshot.workerCodeSha256);
  const code = await assembleOnce(envelope.runtimeKey, async () => {
    const built = modulesFor(snapshot, false, entrypoint);
    const deploymentId = envelope.loaderKey.split("/")[2];
    return {
      compatibilityDate: snapshot.compatibilityDate,
      compatibilityFlags: snapshot.compatibilityFlags,
      mainModule: built.mainModule,
      modules: built.modules,
      env: tenantEnv(snapshot, ctx, deploymentId, doPolicy(env)),
      globalOutbound: ctx.exports.OutboundGateway({
        props: { deploymentId, policyVersion: 1 },
      }),
      limits: PROFILE,
    };
  });
  let cold = false;
  const stub = env.LOADER.get(envelope.runtimeKey, async () => {
    cold = true;
    return code;
  });
  return {
    target: stub.getEntrypoint(entrypoint, { limits: PROFILE }),
    loaderOutcome: () => cold ? "cold" : "warm",
  };
}

async function handleQueue(request, env, ctx) {
  try {
    const payload = await request.json();
    if (!payload || typeof payload !== "object"
        || typeof payload.queueName !== "string" || payload.queueName.length < 1
        || payload.queueName.length > 128 || !Array.isArray(payload.messages)
        || payload.messages.length < 1 || payload.messages.length > MAX_QUEUE_MESSAGES) {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    let totalBytes = 0;
    const messages = payload.messages.map((message) => {
      if (!message || typeof message.id !== "string"
          || !Number.isSafeInteger(message.timestampMs) || message.timestampMs < 0
          || !Number.isSafeInteger(message.attempts)
          || message.attempts < 1 || message.attempts > 101) {
        throw bindingError("QUEUE_DISPOSITION_INVALID");
      }
      const decoded = customEventMessageBody(message);
      totalBytes += decoded.byteLength;
      if (totalBytes > MAX_QUEUE_BATCH_BYTES) {
        throw bindingError("QUEUE_DISPOSITION_INVALID");
      }
      return {
        id: message.id,
        timestamp: new Date(message.timestampMs),
        attempts: message.attempts,
        body: decoded.body,
      };
    });
    const loaded = await customEventTarget(request, env, ctx);
    const result = await loaded.target.queue(payload.queueName, messages);
    const response = Response.json(result);
    response.headers.set("x-open-compute-loader-outcome", loaded.loaderOutcome());
    return response;
  } catch (error) {
    const stable = error && error.stableCode;
    return stableError(stable || "QUEUE_CUSTOM_EVENT_UNSUPPORTED", stable ? 422 : 500, null);
  }
}

async function handleScheduled(request, env, ctx) {
  try {
    const payload = await request.json();
    if (!payload || typeof payload !== "object"
        || !Number.isSafeInteger(payload.scheduledTimeMs) || payload.scheduledTimeMs < 0
        || typeof payload.cron !== "string" || payload.cron.length < 1
        || payload.cron.length > 256) {
      throw bindingError("CRON_EXPRESSION_INVALID");
    }
    const loaded = await customEventTarget(request, env, ctx);
    const result = await loaded.target.scheduled({
      scheduledTime: new Date(payload.scheduledTimeMs),
      cron: payload.cron,
    });
    const response = Response.json(result);
    response.headers.set("x-open-compute-loader-outcome", loaded.loaderOutcome());
    return response;
  } catch (error) {
    const stable = error && error.stableCode;
    return stableError(stable || "CRON_CUSTOM_EVENT_UNSUPPORTED", stable ? 422 : 500, null);
  }
}

function moduleExportsDurableObjectClass(modules, className) {
  const patterns = [
    new RegExp(`export\\s+class\\s+${className}\\b`),
    new RegExp(`export\\s+(?:const|let|var)\\s+${className}\\s*=\\s*class\\b`),
    new RegExp(`export\\s*\\{[^}]*\\b${className}\\b[^}]*\\}`),
  ];
  return modules.some((module) => {
    if (module.type !== "esModule") return false;
    const source = new TextDecoder().decode(bytes(module.bytesBase64));
    return patterns.some((pattern) => pattern.test(source));
  });
}

async function validateDurableObjectClass(request, env) {
  const className = request.headers.get("x-open-compute-entrypoint") || "";
  const envelope = assertEnvelope(request, true, className);
  const internalToken = request.headers.get(TOKEN_HEADER) || "";
  const snapshot = await resolveSnapshot(env, envelope, true, false, internalToken);
  if (!moduleExportsDurableObjectClass(snapshot.modules, className)) {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
  const built = modulesFor(snapshot, false, className, true);
  const code = {
    compatibilityDate: snapshot.compatibilityDate,
    compatibilityFlags: snapshot.compatibilityFlags,
    mainModule: built.mainModule,
    modules: built.modules,
    env: {},
    globalOutbound: null,
    limits: PROFILE,
  };
  try {
    const loaded = env.LOADER.get(`validate-do/${envelope.runtimeKey}`, () => code);
    loaded.getDurableObjectClass(className);
    return new Response(null, { status: 204 });
  } catch {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
}

export default {
  async fetch(request, env, ctx) {
    const path = new URL(request.url).pathname;
    if (request.method === "POST" && ["/internal/workflow", "/internal/validate-workflow", "/internal/workflow-v2", "/internal/validate-workflow-v2"].includes(path)) {
      return handleWorkflow(request, env, ctx, path.includes("/validate-workflow"), path.endsWith("-v2") ? 2 : 1);
    }
    if (request.method === "POST" && path === "/internal/dispatch") return handle(request, env, ctx, false);
    if (request.method === "POST" && path === "/internal/queue") {
      return handleQueue(request, env, ctx);
    }
    if (request.method === "POST" && path === "/internal/scheduled") {
      return handleScheduled(request, env, ctx);
    }
    if (request.method === "POST" && path === "/internal/validate") return handle(request, env, ctx, true);
    if (request.method === "POST" && path === "/internal/validate-do") {
      return validateDurableObjectClass(request, env);
    }
    return new Response(null, { status: 404 });
  },
};

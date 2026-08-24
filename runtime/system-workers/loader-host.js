import { WorkerEntrypoint } from "cloudflare:workers";

const SOURCE_PATH = "/internal/runtime/v1/deployments/resolve";
const TOKEN_HEADER = "x-open-compute-internal-token";
const BINDING_TOKEN_HEADER = "x-open-compute-binding-token";
const BINDING_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+json";
const BINDING_FRAME_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+frame";
const MAX_BINDING_KEY_BYTES = 512;
const MAX_KV_VALUE_BYTES = 25 * 1024 * 1024;
const MAX_KV_KEYS = 100;
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
  "forwarded",
  "x-forwarded-for",
  "x-forwarded-host",
  "x-forwarded-proto",
];

const PROFILE = Object.freeze({ cpuMs: 50, subRequests: 16 });

function currentStartupGeneration() {
  if (!startupGeneration) startupGeneration = crypto.randomUUID();
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

function bytes(base64) {
  const binary = atob(base64);
  const value = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) value[i] = binary.charCodeAt(i);
  return value;
}

function moduleValue(module) {
  const raw = bytes(module.bytesBase64);
  switch (module.type) {
    case "esModule":
      return { js: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "commonJsModule":
      return { cjs: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "text":
      return { text: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "json":
      return { json: JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw)) };
    case "data":
      return { data: raw };
    case "wasm":
      return { wasm: raw };
    default:
      throw new Error("unsupported module representation");
  }
}

function modulesFor(snapshot, validation, validationEntrypoint) {
  const modules = {};
  for (const module of snapshot.modules) modules[module.name] = moduleValue(module);
  if (validation) {
    const wrapper = "__open_compute_validation__.js";
    const exportName = validationEntrypoint || "default";
    modules[wrapper] = { js: `import * as tenant from ${JSON.stringify(snapshot.mainModule)};\nif (!(${JSON.stringify(exportName)} in tenant)) throw new Error(\"missing entrypoint\");\nexport default { fetch() { return new Response(\"open-compute-validation-v1\"); } };` };
    return { modules, mainModule: wrapper };
  }
  return { modules, mainModule: snapshot.mainModule };
}

function assertEnvelope(request, validation, validationEntrypoint) {
  const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
  const expected = request.headers.get("x-open-compute-worker-code-sha256") || "";
  const parts = loaderKey.split("/");
  if (parts.length !== 3 || parts.some((part) => !/^[0-9a-f]{8}-[0-9a-f-]{27}$/.test(part))) {
    throw new Error("invalid loader key");
  }
  if (!/^[0-9a-f]{64}$/.test(expected)) throw new Error("invalid descriptor hash");
  if (validationEntrypoint && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(validationEntrypoint)) {
    throw new Error("invalid entrypoint");
  }
  return {
    loaderKey,
    expected,
    runtimeKey: `${validation ? "validate" : "runtime"}/${loaderKey}${validation ? `/${expected}/${validationEntrypoint || "default"}` : ""}`,
  };
}

async function resolveSnapshot(env, envelope, validation, probe, internalToken) {
  const response = await env.RUNTIME_SOURCE.fetch(`http://runtime-source${SOURCE_PATH}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      [TOKEN_HEADER]: internalToken,
    },
    body: JSON.stringify({
      startupGeneration: currentStartupGeneration(),
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

function bindingError(code) {
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

function trustedBindingProps(descriptor, deploymentId) {
  return Object.freeze({
    bindingId: descriptor.bindingId,
    deploymentId,
    descriptorSha256: descriptor.descriptorSha256,
    resourceSpecGeneration: descriptor.resourceSpecGeneration,
    permissions: Object.freeze({
      read: descriptor.permissions.read === true,
      write: descriptor.permissions.write === true,
    }),
  });
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

function makeBinding(ctx, descriptor, deploymentId) {
  const capability = `${descriptor.kind}@${descriptor.capabilityVersion}`;
  switch (capability) {
    case "kv_namespace@1":
      return ctx.exports.KVNamespace({
        props: trustedBindingProps(descriptor, deploymentId),
      });
    default:
      throw bindingError("BINDING_CAPABILITY_UNSUPPORTED");
  }
}

function tenantEnv(snapshot, ctx, deploymentId) {
  const env = { ...snapshot.env };
  for (const descriptor of snapshot.bindings || []) {
    if (Object.prototype.hasOwnProperty.call(env, descriptor.name)) {
      throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    }
    env[descriptor.name] = makeBinding(ctx, descriptor, deploymentId);
  }
  return env;
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
    const validationEntrypoint = validation ? (request.headers.get("x-open-compute-entrypoint") || undefined) : undefined;
    const envelope = assertEnvelope(request, validation, validationEntrypoint);
    const internalToken = request.headers.get(TOKEN_HEADER) || "";
    // Resolve and verify on every path, including a warm WorkerLoader key.
    const snapshot = await resolveSnapshot(env, envelope, validation, Boolean(validationEntrypoint), internalToken);
    const prior = seenHashes.get(envelope.runtimeKey);
    if (prior && prior !== snapshot.workerCodeSha256) {
      const error = new Error("DEPLOYMENT_INVARIANT_VIOLATION");
      error.stableCode = "DEPLOYMENT_INVARIANT_VIOLATION";
      throw error;
    }
    seenHashes.set(envelope.runtimeKey, snapshot.workerCodeSha256);
    const code = await assembleOnce(envelope.runtimeKey, async () => {
      const built = modulesFor(snapshot, validation, validationEntrypoint);
      const deploymentId = envelope.loaderKey.split("/")[2];
      return {
        compatibilityDate: snapshot.compatibilityDate,
        compatibilityFlags: snapshot.compatibilityFlags,
        mainModule: built.mainModule,
        modules: built.modules,
        env: validation ? {} : tenantEnv(snapshot, ctx, deploymentId),
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
    const entrypoint = validation ? undefined : (request.headers.get("x-open-compute-entrypoint") || undefined);
    const target = stub.getEntrypoint(entrypoint, { limits: PROFILE });
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

export default {
  async fetch(request, env, ctx) {
    const path = new URL(request.url).pathname;
    if (request.method === "POST" && path === "/internal/dispatch") return handle(request, env, ctx, false);
    if (request.method === "POST" && path === "/internal/validate") return handle(request, env, ctx, true);
    return new Response(null, { status: 404 });
  },
};

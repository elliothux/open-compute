import { base64Bytes, hex, hmacSha256, randomBytes, utf8 } from "./__open_compute_do_id_codec__.js";

const namespaceState = new WeakMap();
const idState = new WeakMap();
const stubState = new WeakMap();
const FORBIDDEN_RPC = new Set(["constructor", "prototype", "__proto__", "then", "fetch"]);
const PUBLIC_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const ID = /^[0-9a-f]{64}$/;
const encodeBase64 = btoa.bind(globalThis);
const decodeBase64 = atob.bind(globalThis);

function failure(code, type = Error) {
  const error = new type(code);
  error.stableCode = code;
  error.stack = `${error.name}: ${code}`;
  return error;
}

function assertOptions(options) {
  if (options === undefined) return;
  if (!options || typeof options !== "object" || Array.isArray(options)) {
    throw failure("DO_PLACEMENT_OPTION_UNSUPPORTED", TypeError);
  }
  if (Object.keys(options).length !== 0) {
    throw failure("DO_PLACEMENT_OPTION_UNSUPPORTED", TypeError);
  }
}

function assertName(name, maxBytes) {
  if (typeof name !== "string") throw failure("DO_ID_INVALID", TypeError);
  const bytes = utf8(name);
  if (bytes.byteLength > maxBytes) throw failure("DO_ID_INVALID", TypeError);
  return bytes;
}

function assertPlain(value, seen = new WeakSet()) {
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number" && Number.isFinite(value)) return;
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) return;
  if (!value || typeof value !== "object" || value instanceof Promise || value instanceof ReadableStream) {
    throw failure("DO_RPC_UNSUPPORTED", TypeError);
  }
  const prototype = Object.getPrototypeOf(value);
  if (!Array.isArray(value) && prototype !== Object.prototype && prototype !== null) {
    throw failure("DO_RPC_UNSUPPORTED", TypeError);
  }
  if (seen.has(value)) throw failure("DO_RPC_UNSUPPORTED", TypeError);
  seen.add(value);
  for (const item of Array.isArray(value) ? value : Object.values(value)) assertPlain(item, seen);
  seen.delete(value);
}

function binaryBase64(value) {
  const bytes = value instanceof ArrayBuffer
    ? new Uint8Array(value)
    : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return encodeBase64(binary);
}

function encodeWire(value) {
  if (value === null) return ["z"];
  if (typeof value === "string") return ["s", value];
  if (typeof value === "boolean") return ["b", value];
  if (typeof value === "number" && Number.isFinite(value)) return ["n", value];
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
    return ["x", binaryBase64(value)];
  }
  if (Array.isArray(value)) return ["a", value.map(encodeWire)];
  return ["o", Object.entries(value).map(([key, item]) => [key, encodeWire(item)])];
}

function decodeWire(value) {
  if (!Array.isArray(value) || typeof value[0] !== "string") {
    throw failure("DO_RPC_UNSUPPORTED", TypeError);
  }
  switch (value[0]) {
    case "z": return null;
    case "s": if (typeof value[1] === "string") return value[1]; break;
    case "b": if (typeof value[1] === "boolean") return value[1]; break;
    case "n": if (typeof value[1] === "number" && Number.isFinite(value[1])) return value[1]; break;
    case "x": {
      if (typeof value[1] !== "string") break;
      const binary = decodeBase64(value[1]);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      return bytes.buffer;
    }
    case "a": if (Array.isArray(value[1])) return value[1].map(decodeWire); break;
    case "o": {
      if (!Array.isArray(value[1])) break;
      const result = Object.create(null);
      for (const entry of value[1]) {
        if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== "string") {
          throw failure("DO_RPC_UNSUPPORTED", TypeError);
        }
        Object.defineProperty(result, entry[0], {
          value: decodeWire(entry[1]), enumerable: true, writable: true, configurable: true,
        });
      }
      return result;
    }
  }
  throw failure("DO_RPC_UNSUPPORTED", TypeError);
}

export class DurableObjectId {
  constructor(marker, value, name) {
    if (marker !== idState) throw failure("DO_ID_INVALID", TypeError);
    idState.set(this, Object.freeze({ value, name }));
    Object.freeze(this);
  }

  get name() { return idState.get(this).name; }
  toString() { return idState.get(this).value; }
  equals(other) { return idState.has(other) && idState.get(other).value === this.toString(); }
}

function makeId(value, name) {
  return new DurableObjectId(idState, value, name);
}

function stubProxy(id, raw) {
  const target = Object.create(DurableObjectStub.prototype);
  const state = Object.freeze({ id, raw });
  stubState.set(target, state);
  const proxy = new Proxy(target, {
    get(owner, property, receiver) {
      if (property === "then") return undefined;
      const value = Reflect.get(owner, property, receiver);
      if (value !== undefined || typeof property !== "string") return value;
      if (FORBIDDEN_RPC.has(property) || !PUBLIC_METHOD.test(property)) {
        throw failure("DO_RPC_UNSUPPORTED", TypeError);
      }
      return async (...args) => {
        assertPlain(args);
        try {
          return decodeWire(await raw.dispatchRpc(id.toString(), property, encodeWire(args)));
        } catch (error) {
          const code = /\b(DO_[A-Z_]+)\b/.exec(String(error && error.message || error));
          throw failure(code ? code[1] : "DO_RUNTIME_EXCEPTION");
        }
      };
    },
  });
  stubState.set(proxy, state);
  return proxy;
}

export class DurableObjectStub {
  get id() { return stubState.get(this).id; }
  get name() { return this.id.name; }

  async fetch(input, init) {
    const state = stubState.get(this);
    let request;
    try {
      request = input instanceof Request && init === undefined ? input : new Request(input, init);
    } catch {
      throw failure("DO_RPC_UNSUPPORTED", TypeError);
    }
    try {
      const headers = new Headers(request.headers);
      headers.set("x-open-compute-do-method", request.method);
      headers.set("x-open-compute-do-url", request.url);
      const transport = {
        method: request.method,
        headers,
        body: request.body,
        redirect: "manual",
      };
      if (request.method === "GET" || request.method === "HEAD") delete transport.body;
      return await state.raw.fetch(new Request(
        `https://do-transport.invalid/${state.id.toString()}`,
        transport,
      ));
    } catch (error) {
      const code = /\b(DO_[A-Z_]+)\b/.exec(String(error && error.message || error));
      throw failure(code ? code[1] : "DO_RUNTIME_EXCEPTION");
    }
  }
}

export class DurableObjectNamespace {
  constructor(composite) {
    if (!composite || composite.schemaVersion !== 1 || typeof composite.namespacePrefix !== "string"
        || !/^[0-9a-f]{16}$/.test(composite.namespacePrefix)
        || typeof composite.namespaceNameKey !== "string" || !composite.transport
        || !Number.isSafeInteger(composite.maxObjectNameBytes)
        || composite.maxObjectNameBytes < 1 || composite.maxObjectNameBytes > 1024) {
      throw failure("DO_NAMESPACE_NOT_FOUND");
    }
    namespaceState.set(this, Object.freeze({
      prefix: composite.namespacePrefix,
      key: base64Bytes(composite.namespaceNameKey),
      maxNameBytes: composite.maxObjectNameBytes,
      raw: composite.transport,
    }));
    Object.freeze(this);
  }

  idFromName(name) {
    const state = namespaceState.get(this);
    const bytes = assertName(name, state.maxNameBytes);
    const payload = hmacSha256(state.key, bytes).subarray(0, 24);
    return makeId(state.prefix + hex(payload), name);
  }

  newUniqueId(options) {
    assertOptions(options);
    const state = namespaceState.get(this);
    return makeId(state.prefix + hex(randomBytes(24)), undefined);
  }

  idFromString(value) {
    const state = namespaceState.get(this);
    if (typeof value !== "string" || !ID.test(value)) throw failure("DO_ID_INVALID", TypeError);
    const canonical = value.toLowerCase();
    if (!canonical.startsWith(state.prefix)) throw failure("DO_ID_INVALID", TypeError);
    return makeId(canonical, undefined);
  }

  get(id, options) {
    assertOptions(options);
    if (!idState.has(id)) throw failure("DO_ID_INVALID", TypeError);
    const canonical = id.toString();
    if (!canonical.startsWith(namespaceState.get(this).prefix)) {
      throw failure("DO_ID_INVALID", TypeError);
    }
    return stubProxy(id, namespaceState.get(this).raw);
  }

  getByName(name, options) {
    return this.get(this.idFromName(name), options);
  }
}

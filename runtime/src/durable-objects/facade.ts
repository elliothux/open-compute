import { base64Bytes, hex, hmacSha256, randomBytes, utf8 } from "./id-codec.js";
import type { DoNamespaceCapability, DoPlainValue, DoRawTransport, DoWireValue } from "./protocol.js";

interface IdState { value: string; name: string | undefined }
interface StubState { id: DurableObjectId; raw: DoRawTransport; queue: { tail: Promise<void> } }
const namespaceState = new WeakMap<object, { prefix: string; key: Uint8Array; maxNameBytes: number; raw: DoRawTransport }>();
const idState = new WeakMap<object, IdState>();
const stubState = new WeakMap<object, StubState>();
const FORBIDDEN_RPC = new Set(["constructor", "prototype", "__proto__", "then", "fetch"]);
const PUBLIC_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const ID = /^[0-9a-f]{64}$/;
const encodeBase64 = btoa.bind(globalThis);
const decodeBase64 = atob.bind(globalThis);

function failure(code: string, type: ErrorConstructor = Error) {
  const error = Object.assign(new type(code), { stableCode: code });
  error.stack = `${error.name}: ${code}`;
  return error;
}

function assertOptions(options: unknown) {
  if (options === undefined) return;
  if (!options || typeof options !== "object" || Array.isArray(options)) {
    throw failure("DO_PLACEMENT_OPTION_UNSUPPORTED", TypeError);
  }
  if (Object.keys(options).length !== 0) {
    throw failure("DO_PLACEMENT_OPTION_UNSUPPORTED", TypeError);
  }
}

function assertName(name: unknown, maxBytes: number): Uint8Array {
  if (typeof name !== "string") throw failure("DO_ID_INVALID", TypeError);
  const bytes = utf8(name);
  if (bytes.byteLength > maxBytes) throw failure("DO_ID_INVALID", TypeError);
  return bytes;
}

function assertPlain(value: unknown, seen = new WeakSet<object>()): asserts value is DoPlainValue {
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

function binaryBase64(value: ArrayBuffer | ArrayBufferView): string {
  const bytes = value instanceof ArrayBuffer
    ? new Uint8Array(value)
    : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return encodeBase64(binary);
}

function encodeWire(value: DoPlainValue): DoWireValue {
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

function decodeWire(value: unknown): DoPlainValue {
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
      const result: Record<string, DoPlainValue> = Object.create(null);
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
  constructor(marker: WeakMap<object, IdState>, value: string, name: string | undefined) {
    if (marker !== idState) throw failure("DO_ID_INVALID", TypeError);
    idState.set(this, Object.freeze({ value, name }));
    Object.freeze(this);
  }

  get name() { return idState.get(this)!.name; }
  toString() { return idState.get(this)!.value; }
  equals(other: unknown) {
    const state = other !== null && (typeof other === "object" || typeof other === "function") ? idState.get(other) : undefined;
    return state !== undefined && state.value === this.toString();
  }
}

function makeId(value: string, name: string | undefined) {
  return new DurableObjectId(idState, value, name);
}

function enqueueStubOperation<T>(state: StubState, operation: () => Promise<T>): Promise<T> {
  const result = state.queue.tail.then(operation);
  state.queue.tail = result.then(() => undefined, () => undefined);
  return result;
}

function stubProxy(id: DurableObjectId, raw: DoRawTransport): DurableObjectStub {
  const target = new DurableObjectStub();
  const state = Object.freeze({ id, raw, queue: { tail: Promise.resolve() } });
  stubState.set(target, state);
  const proxy = new Proxy(target, {
    get(owner, property, receiver) {
      if (property === "then") return undefined;
      const value: unknown = Reflect.get(owner, property, receiver);
      if (value !== undefined || typeof property !== "string") return value;
      if (FORBIDDEN_RPC.has(property) || !PUBLIC_METHOD.test(property)) {
        throw failure("DO_RPC_UNSUPPORTED", TypeError);
      }
      return async (...args: unknown[]) => {
        assertPlain(args);
        try {
          return decodeWire(await enqueueStubOperation(
            state,
            () => raw.dispatchRpc(id.toString(), property, encodeWire(args)),
          ));
        } catch (error) {
          const code = /\b(DO_[A-Z_]+)\b/.exec(String(error instanceof Error ? error.message : error));
          throw failure(code ? code[1]! : "DO_RUNTIME_EXCEPTION");
        }
      };
    },
  });
  stubState.set(proxy, state);
  return proxy;
}

export class DurableObjectStub {
  get id() { return stubState.get(this)!.id; }
  get name() { return this.id.name; }

  async fetch(input: RequestInfo | URL, init?: RequestInit) {
    const state = stubState.get(this)!;
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
      const transport: RequestInit = {
        method: request.method,
        headers,
        body: request.body,
        redirect: "manual",
      };
      if (request.method === "GET" || request.method === "HEAD") delete transport.body;
      const outbound = new Request(
        `https://do-transport.invalid/${state.id.toString()}`,
        transport,
      );
      return await enqueueStubOperation(state, () => state.raw.fetch(outbound));
    } catch (error) {
      const code = /\b(DO_[A-Z_]+)\b/.exec(String(error instanceof Error ? error.message : error));
      throw failure(code ? code[1]! : "DO_RUNTIME_EXCEPTION");
    }
  }
}

export class DurableObjectNamespace {
  constructor(composite: unknown) {
    if (!namespaceCapability(composite)) {
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

  idFromName(name: string) {
    const state = namespaceState.get(this)!;
    const bytes = assertName(name, state.maxNameBytes);
    const payload = hmacSha256(state.key, bytes).subarray(0, 24);
    return makeId(state.prefix + hex(payload), name);
  }

  newUniqueId(options?: unknown) {
    assertOptions(options);
    const state = namespaceState.get(this)!;
    return makeId(state.prefix + hex(randomBytes(24)), undefined);
  }

  idFromString(value: string) {
    const state = namespaceState.get(this)!;
    if (typeof value !== "string" || !ID.test(value)) throw failure("DO_ID_INVALID", TypeError);
    const canonical = value.toLowerCase();
    if (!canonical.startsWith(state.prefix)) throw failure("DO_ID_INVALID", TypeError);
    return makeId(canonical, undefined);
  }

  get(id: DurableObjectId, options?: unknown) {
    assertOptions(options);
    if (!idState.has(id)) throw failure("DO_ID_INVALID", TypeError);
    const canonical = id.toString();
    if (!canonical.startsWith(namespaceState.get(this)!.prefix)) {
      throw failure("DO_ID_INVALID", TypeError);
    }
    return stubProxy(id, namespaceState.get(this)!.raw);
  }

  getByName(name: string, options?: unknown) {
    return this.get(this.idFromName(name), options);
  }
}

function namespaceCapability(value: unknown): value is DoNamespaceCapability {
  if (value === null || typeof value !== "object"
      || !("schemaVersion" in value) || value.schemaVersion !== 1
      || !("namespacePrefix" in value) || typeof value.namespacePrefix !== "string"
      || !/^[0-9a-f]{16}$/.test(value.namespacePrefix)
      || !("namespaceNameKey" in value) || typeof value.namespaceNameKey !== "string"
      || !("maxObjectNameBytes" in value) || typeof value.maxObjectNameBytes !== "number"
      || !Number.isSafeInteger(value.maxObjectNameBytes)
      || value.maxObjectNameBytes < 1 || value.maxObjectNameBytes > 1024
      || !("transport" in value) || value.transport === null || typeof value.transport !== "object") return false;
  const raw = value.transport;
  return "dispatchRpc" in raw && typeof raw.dispatchRpc === "function"
    && "fetch" in raw && typeof raw.fetch === "function";
}

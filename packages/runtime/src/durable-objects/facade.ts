import { waitUntil } from "cloudflare:workers";
import { socketAuthorityWire } from "../sockets/tunnel.js";
import { base64Bytes, hex, hmacSha256, randomBytes, utf8 } from "./id-codec.js";
import type { DoNamespaceCapability, DoRawTransport, DoRpcResultProvider } from "./protocol.js";

interface IdState { value: string; name: string | undefined; jurisdiction: string | undefined }
interface NamespaceState {
  prefix: string;
  key: Uint8Array;
  maxNameBytes: number;
  raw: DoRawTransport;
  jurisdiction: string | undefined;
}
interface StubState {
  id: DurableObjectId;
  raw: DoRawTransport;
  order: StubOrder;
  rpcWrappers: WeakMap<object, object>;
}
interface StubOrder {
  channelId: string;
  next: number;
  inFlight: number;
  lastUsed: number;
  startTail: Promise<void>;
}
const namespaceState = new WeakMap<object, NamespaceState>();
const idState = new WeakMap<object, IdState>();
const stubState = new WeakMap<object, StubState>();
const stubOrders = new WeakMap<object, Map<string, StubOrder>>();
const FORBIDDEN_RPC = new Set([
  "constructor", "prototype", "__proto__", "then", "dup", "fetch", "connect", "alarm",
  "webSocketMessage", "webSocketClose", "webSocketError",
]);
const LOCAL_STUB_MEMBERS = new Set(["id", "name", "fetch", "connect"]);
const ID = /^[0-9a-f]{64}$/;
const ID_PREFIX_HEX_LENGTH = 16;
const ID_BODY_BYTES = 15;
const ID_TAG_BYTES = 8;
const ID_FORMAT_BASE = 0xa0;
const JURISDICTIONS = new Set(["eu", "fedramp", "fedramp-high", "us"]);
const JURISDICTION_CODES = new Map<string, number>([
  ["eu", 1], ["fedramp", 2], ["fedramp-high", 3], ["us", 4],
]);
const JURISDICTIONS_BY_CODE = new Map<number, string>(
  [...JURISDICTION_CODES].map(([name, code]) => [code, name]),
);
const LOCATION_HINTS = new Set([
  "wnam", "enam", "sam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc", "afr", "me",
]);
const ROUTING_MODES = new Set(["primary-only"]);
const ORDER_IDLE_MS = 60_000;
const MAX_STUB_ORDERS = 65_536;

function failure(code: string, type: ErrorConstructor = Error) {
  const error = Object.assign(new type(code), { stableCode: code });
  error.stack = `${error.name}: ${code}`;
  return error;
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function enumOption(value: unknown, allowed: Set<string>, code: string): string {
  if (typeof value !== "string" || !allowed.has(value)) throw failure(code, TypeError);
  return value;
}

function getOptions(options: unknown): { locationHint?: string; routingMode?: string } {
  if (options === undefined) return {};
  if (!record(options)) throw failure("DO_ID_INVALID", TypeError);
  const output: { locationHint?: string; routingMode?: string } = {};
  if (options.locationHint !== undefined) {
    output.locationHint = enumOption(options.locationHint, LOCATION_HINTS, "DO_ID_INVALID");
  }
  if (options.routingMode !== undefined) {
    output.routingMode = enumOption(options.routingMode, ROUTING_MODES, "DO_ID_INVALID");
  }
  return output;
}

function uniqueIdOptions(options: unknown): { jurisdiction?: string } {
  if (options === undefined) return {};
  if (!record(options)) throw failure("DO_ID_INVALID", TypeError);
  if (options.jurisdiction === undefined || options.jurisdiction === null) return {};
  return { jurisdiction: enumOption(options.jurisdiction, JURISDICTIONS, "DO_ID_INVALID") };
}

function parseJurisdiction(value: unknown): string | undefined {
  if (value === undefined || value === null) return undefined;
  return enumOption(value, JURISDICTIONS, "DO_ID_INVALID");
}

function hexBytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) return false;
  let difference = 0;
  for (let index = 0; index < left.byteLength; index += 1) {
    difference |= left[index]! ^ right[index]!;
  }
  return difference === 0;
}

function idPayload(key: Uint8Array, body: Uint8Array, requested: string | undefined): Uint8Array {
  if (body.byteLength !== ID_BODY_BYTES) throw failure("DO_ID_INVALID", TypeError);
  const payload = new Uint8Array(1 + ID_BODY_BYTES + ID_TAG_BYTES);
  payload[0] = ID_FORMAT_BASE + (requested === undefined ? 0 : JURISDICTION_CODES.get(requested)!);
  payload.set(body, 1);
  payload.set(
    hmacSha256(key, payload.subarray(0, 1 + ID_BODY_BYTES)).subarray(0, ID_TAG_BYTES),
    1 + ID_BODY_BYTES,
  );
  return payload;
}

function namedBody(key: Uint8Array, name: Uint8Array, requested: string | undefined): Uint8Array {
  const input = new Uint8Array(2 + name.byteLength);
  input[0] = 0x6e;
  input[1] = requested === undefined ? 0 : JURISDICTION_CODES.get(requested)!;
  input.set(name, 2);
  return hmacSha256(key, input).subarray(0, ID_BODY_BYTES);
}

function decodeId(
  state: Pick<NamespaceState, "prefix" | "key" | "jurisdiction">,
  value: unknown,
): { value: string; jurisdiction: string | undefined } {
  if (typeof value !== "string" || !ID.test(value) || !value.startsWith(state.prefix)) {
    throw failure("DO_ID_INVALID", TypeError);
  }
  const payload = hexBytes(value.slice(ID_PREFIX_HEX_LENGTH));
  const code = payload[0]! - ID_FORMAT_BASE;
  const decodedJurisdiction = code === 0 ? undefined : JURISDICTIONS_BY_CODE.get(code);
  if (code < 0 || code > JURISDICTION_CODES.size || (code !== 0 && decodedJurisdiction === undefined)) {
    throw failure("DO_ID_INVALID", TypeError);
  }
  const content = payload.subarray(0, 1 + ID_BODY_BYTES);
  const expected = hmacSha256(state.key, content).subarray(0, ID_TAG_BYTES);
  if (!equalBytes(expected, payload.subarray(1 + ID_BODY_BYTES))) {
    throw failure("DO_ID_INVALID", TypeError);
  }
  if (state.jurisdiction !== undefined && state.jurisdiction !== decodedJurisdiction) {
    throw failure("DO_ID_INVALID", TypeError);
  }
  return { value, jurisdiction: decodedJurisdiction };
}

function assertName(name: unknown, maxBytes: number): Uint8Array {
  if (typeof name !== "string") throw failure("DO_ID_INVALID", TypeError);
  const bytes = utf8(name);
  if (bytes.byteLength > maxBytes) throw failure("DO_ID_INVALID", TypeError);
  return bytes;
}

export class DurableObjectId {
  constructor(marker: WeakMap<object, IdState>, value: string, name: string | undefined, jurisdiction: string | undefined) {
    if (marker !== idState) throw failure("DO_ID_INVALID", TypeError);
    idState.set(this, Object.freeze({ value, name, jurisdiction }));
    Object.freeze(this);
  }

  get name() { return idState.get(this)!.name; }
  get jurisdiction() { return idState.get(this)!.jurisdiction; }
  toString() { return idState.get(this)!.value; }
  equals(other: unknown) {
    const state = other !== null && (typeof other === "object" || typeof other === "function") ? idState.get(other) : undefined;
    return state !== undefined && state.value === this.toString();
  }
}

function makeId(value: string, name: string | undefined, jurisdiction: string | undefined) {
  return new DurableObjectId(idState, value, name, jurisdiction);
}

function rpcFailure(error: unknown): Error {
  const message = String(error instanceof Error ? error.message : error);
  const code = /\b(DO_[A-Z_]+)\b/.exec(message);
  return failure(code ? code[1]! : "DO_RUNTIME_EXCEPTION");
}

function object(value: unknown): value is object {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

function callable(value: unknown): value is (...args: unknown[]) => unknown {
  return typeof value === "function";
}

function stubOrder(raw: DoRawTransport, id: string): StubOrder {
  let orders = stubOrders.get(raw);
  if (!orders) {
    orders = new Map();
    stubOrders.set(raw, orders);
  }
  const prior = orders.get(id);
  if (prior) return prior;
  const now = Date.now();
  if (orders.size >= MAX_STUB_ORDERS) {
    for (const [key, order] of orders) {
      if (order.inFlight === 0 && now - order.lastUsed >= ORDER_IDLE_MS) orders.delete(key);
    }
  }
  if (orders.size >= MAX_STUB_ORDERS) throw failure("DO_STORAGE_LIMIT");
  const order = {
    channelId: crypto.randomUUID().replaceAll("-", ""),
    next: 0,
    inFlight: 0,
    lastUsed: now,
    startTail: Promise.resolve(),
  };
  orders.set(id, order);
  return order;
}

function beginOperation(order: StubOrder) {
  const now = Date.now();
  if (order.inFlight === 0 && now - order.lastUsed >= ORDER_IDLE_MS) {
    order.channelId = crypto.randomUUID().replaceAll("-", "");
    order.next = 0;
  }
  if (!Number.isSafeInteger(order.next)) throw failure("DO_STORAGE_LIMIT");
  const call = { channelId: order.channelId, sequence: order.next };
  const immediate = order.inFlight === 0;
  const predecessor = order.startTail;
  let releaseStart: () => void = () => {};
  const startGate = new Promise<void>(resolve => { releaseStart = resolve; });
  order.startTail = predecessor.then(() => startGate);
  order.next += 1;
  order.inFlight += 1;
  order.lastUsed = now;
  let finished = false;
  let startReleased = false;
  const started = (value?: PromiseLike<unknown>) => {
    if (startReleased) return;
    startReleased = true;
    if (value === undefined) {
      releaseStart();
      return;
    }
    Promise.resolve(value).then(releaseStart, releaseStart);
  };
  return {
    ...call,
    immediate,
    predecessor,
    started,
    rollback() {
      if (finished || order.channelId !== call.channelId || order.next !== call.sequence + 1) {
        return false;
      }
      finished = true;
      started();
      order.next -= 1;
      order.inFlight -= 1;
      order.lastUsed = Date.now();
      return true;
    },
    done() {
      if (finished) return;
      finished = true;
      started();
      order.inFlight -= 1;
      order.lastUsed = Date.now();
    },
  };
}

interface DeferredRpcValue { value: unknown }

function deferredRpcProvider(state: StubState, launched: Promise<DeferredRpcValue>): unknown {
  const target = () => undefined;
  return new Proxy(target, {
    get(_owner, property) {
      if (property === "then") {
        return (fulfilled?: unknown, rejected?: unknown) => launched
          .then(holder => holder.value)
          .then(
            callable(fulfilled)
              ? (resolved: unknown) => fulfilled(sanitizeResolved(state, resolved))
              : undefined,
            (error: unknown) => {
              const safe = rpcFailure(error);
              if (callable(rejected)) return rejected(safe);
              throw safe;
            },
          );
      }
      if (property === Symbol.dispose) {
        return () => waitUntil(launched.then(holder => {
          if (!object(holder.value)) return;
          const dispose: unknown = Reflect.get(holder.value, Symbol.dispose, holder.value);
          if (callable(dispose)) Reflect.apply(dispose, holder.value, []);
        }).catch(() => undefined));
      }
      const child = launched.then(holder => {
        if (!object(holder.value)) return { value: undefined };
        return { value: Reflect.get(holder.value, property, holder.value) };
      });
      return deferredRpcProvider(state, child);
    },
    apply(_owner, _receiver, args) {
      const child = launched.then(holder => {
        if (!callable(holder.value)) throw failure("DO_RUNTIME_EXCEPTION");
        return { value: Reflect.apply(holder.value, holder.value, args) };
      });
      return deferredRpcProvider(state, child);
    },
  });
}

function rpcStub(value: object): boolean {
  try {
    return callable(Reflect.get(value, "dup")) && callable(Reflect.get(value, Symbol.dispose));
  } catch {
    return false;
  }
}

function sanitizeResolved(
  state: StubState,
  value: unknown,
  seen = new WeakMap<object, object>(),
): unknown {
  if (!object(value)) return value;
  if (rpcStub(value)) return protectProvider(state, value);
  if (value instanceof Date || value instanceof Error || value instanceof RegExp
      || value instanceof ArrayBuffer || ArrayBuffer.isView(value)
      || value instanceof Headers || value instanceof Request || value instanceof Response
      || value instanceof ReadableStream || value instanceof WritableStream) return value;
  const prior = seen.get(value);
  if (prior) return prior;
  if (Array.isArray(value)) {
    const output: unknown[] = [];
    seen.set(value, output);
    for (const item of value) output.push(sanitizeResolved(state, item, seen));
    return output;
  }
  if (value instanceof Map) {
    const output = new Map<unknown, unknown>();
    seen.set(value, output);
    for (const [key, item] of value) {
      output.set(sanitizeResolved(state, key, seen), sanitizeResolved(state, item, seen));
    }
    return output;
  }
  if (value instanceof Set) {
    const output = new Set<unknown>();
    seen.set(value, output);
    for (const item of value) output.add(sanitizeResolved(state, item, seen));
    return output;
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return value;
  const output = Object.create(prototype) as Record<string, unknown>;
  seen.set(value, output);
  for (const [key, item] of Object.entries(value)) output[key] = sanitizeResolved(state, item, seen);
  return output;
}

function protectProvider(state: StubState, value: unknown): unknown {
  if (!object(value)) return value;
  const prior = state.rpcWrappers.get(value);
  if (prior) return prior;
  const wrapper = new Proxy(value, {
    get(target, property) {
      try {
        const member: unknown = Reflect.get(target, property, target);
        if (property === "then") {
          if (!callable(member)) return undefined;
          return (fulfilled?: unknown, rejected?: unknown) => Reflect.apply(member, target, [
            callable(fulfilled)
              ? (resolved: unknown) => fulfilled(sanitizeResolved(state, resolved))
              : fulfilled,
            (error: unknown) => {
              const safe = rpcFailure(error);
              if (callable(rejected)) return rejected(safe);
              throw safe;
            },
          ]);
        }
        if (property === "dup" && callable(member)) {
          return () => protectProvider(state, Reflect.apply(member, target, []));
        }
        if (property === Symbol.dispose && callable(member)) {
          return () => Reflect.apply(member, target, []);
        }
        return protectProvider(state, member);
      } catch (error) {
        throw rpcFailure(error);
      }
    },
    apply(target, _receiver, args) {
      try {
        return protectProvider(state, Reflect.apply(target as (...args: unknown[]) => unknown, target, args));
      } catch (error) {
        throw rpcFailure(error);
      }
    },
  });
  state.rpcWrappers.set(value, wrapper);
  return wrapper;
}

function startProvider(state: StubState, value: unknown): unknown {
  return protectProvider(state, value);
}

function rpcOperation(
  state: StubState,
  kind: "call" | "get",
  member: string,
  args: unknown[],
): unknown {
  const operation = beginOperation(state.order);
  const launch = (): DeferredRpcValue => {
    let started: DoRpcResultProvider;
    try {
      started = state.raw.startRpc(
        state.id.toString(),
        operation.channelId,
        operation.sequence,
        kind,
        member,
        args,
      );
    } catch (error) {
      if (!operation.rollback()) operation.done();
      throw rpcFailure(error);
    }
    operation.started(started);
    let result: unknown;
    try {
      result = started.take();
    } catch (error) {
      operation.done();
      throw rpcFailure(error);
    }
    operation.done();
    waitUntil(started.then(
      holder => holder[Symbol.dispose](),
      () => state.raw.cancelOrder(
        state.id.toString(), operation.channelId, operation.sequence,
      ).then(
        () => undefined,
        () => undefined,
      ),
    ));
    return { value: result };
  };
  if (!operation.immediate) {
    return deferredRpcProvider(state, operation.predecessor.then(launch));
  }
  return startProvider(state, launch().value);
}

function rpcMember(state: StubState, property: string): (...args: unknown[]) => unknown {
  let propertyResult: unknown;
  let propertyStarted = false;
  const getProperty = () => {
    if (!propertyStarted) {
      propertyResult = rpcOperation(state, "get", property, []);
      propertyStarted = true;
    }
    return propertyResult;
  };
  const method = (...args: unknown[]) => rpcOperation(state, "call", property, args);
  return new Proxy(method, {
    get(_target, nested) {
      const result = getProperty();
      if (!object(result)) return undefined;
      return Reflect.get(result, nested);
    },
  });
}

function stubProxy(
  id: DurableObjectId,
  raw: DoRawTransport,
): DurableObjectStub {
  const target = new DurableObjectStub();
  const state: StubState = {
    id,
    raw,
    order: stubOrder(raw, id.toString()),
    rpcWrappers: new WeakMap<object, object>(),
  };
  stubState.set(target, state);
  const proxy = new Proxy(target, {
    get(owner, property, receiver) {
      if (property === "then") return undefined;
      if (typeof property !== "string") return Reflect.get(owner, property, receiver);
      if (LOCAL_STUB_MEMBERS.has(property)) return Reflect.get(owner, property, receiver);
      if (FORBIDDEN_RPC.has(property) || property.startsWith("__openCompute")) {
        throw failure("DO_RPC_UNSUPPORTED", TypeError);
      }
      return rpcMember(state, property);
    },
  });
  stubState.set(proxy, state);
  return proxy;
}

export class DurableObjectStub {
  get id() { return stubState.get(this)!.id; }
  get name() { return this.id.name; }

  connect(address: SocketAddress | string, options?: SocketOptions): Socket {
    const state = stubState.get(this)!;
    const ordered = beginOperation(state.order);
    const operationId = crypto.randomUUID().replaceAll("-", "");
    const prepared = ordered.predecessor.then(() => state.raw.prepareConnect(
      state.id.toString(),
      ordered.channelId,
      ordered.sequence,
      operationId,
      socketAuthorityWire(address),
    ));
    waitUntil(prepared.then(
      () => undefined,
      () => undefined,
    ));
    let socket: Socket;
    try {
      socket = state.raw.connect(`${operationId}.do-transport.invalid:1`, options);
    } catch (error) {
      waitUntil(state.raw.cancelConnect(operationId).then(
        () => state.raw.cancelOrder(state.id.toString(), ordered.channelId, ordered.sequence),
        () => state.raw.cancelOrder(state.id.toString(), ordered.channelId, ordered.sequence),
      ).then(
        ordered.done,
        ordered.done,
      ));
      throw error;
    }
    waitUntil(socket.opened.then(
      () => undefined,
      () => state.raw.cancelConnect(operationId).then(
        () => state.raw.cancelOrder(state.id.toString(), ordered.channelId, ordered.sequence),
        () => state.raw.cancelOrder(state.id.toString(), ordered.channelId, ordered.sequence),
      ).then(
        ordered.done,
        ordered.done,
      ),
    ));
    ordered.started(socket.opened);
    waitUntil(socket.closed.then(ordered.done, ordered.done));
    return socket;
  }

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
      const operation = beginOperation(state.order);
      const outbound = new Request(
        `https://do-transport.invalid/${state.id.toString()}/${operation.channelId}/${operation.sequence}`,
        transport,
      );
      let pending: Promise<Response>;
      try {
        await operation.predecessor;
        pending = state.raw.fetch(outbound);
        operation.started(pending);
      } catch (error) {
        operation.rollback();
        throw error;
      }
      try {
        return await pending;
      } catch (error) {
        await state.raw.cancelOrder(
          state.id.toString(), operation.channelId, operation.sequence,
        ).catch(() => undefined);
        throw error;
      } finally {
        operation.done();
      }
    } catch (error) {
      const code = /\b(DO_[A-Z_]+)\b/.exec(String(error instanceof Error ? error.message : error));
      throw failure(code ? code[1]! : "DO_RUNTIME_EXCEPTION");
    }
  }
}

export class DurableObjectNamespace {
  constructor(
    composite: unknown,
    requestedJurisdiction?: unknown,
  ) {
    if (composite instanceof DurableObjectNamespace) {
      const parent = namespaceState.get(composite);
      if (!parent) throw failure("DO_NAMESPACE_NOT_FOUND");
      const scoped = parseJurisdiction(requestedJurisdiction);
      namespaceState.set(this, Object.freeze({ ...parent, jurisdiction: scoped }));
      Object.freeze(this);
      return;
    }
    if (!namespaceCapability(composite)) {
      throw failure("DO_NAMESPACE_NOT_FOUND");
    }
    const key = base64Bytes(composite.namespaceNameKey);
    if (key.byteLength !== 32) throw failure("DO_NAMESPACE_NOT_FOUND");
    namespaceState.set(this, Object.freeze({
      prefix: composite.namespacePrefix,
      key,
      maxNameBytes: composite.maxObjectNameBytes,
      raw: composite.transport,
      jurisdiction: undefined,
    }));
    Object.freeze(this);
  }

  jurisdiction(value: string) {
    return new DurableObjectNamespace(this, value);
  }

  idFromName(name: string) {
    const state = namespaceState.get(this)!;
    const bytes = assertName(name, state.maxNameBytes);
    const payload = idPayload(
      state.key,
      namedBody(state.key, bytes, state.jurisdiction),
      state.jurisdiction,
    );
    return makeId(state.prefix + hex(payload), name, state.jurisdiction);
  }

  newUniqueId(options?: unknown) {
    const requested = uniqueIdOptions(options);
    const state = namespaceState.get(this)!;
    const jurisdiction = requested.jurisdiction ?? state.jurisdiction;
    if (requested.jurisdiction !== undefined && state.jurisdiction !== undefined
        && requested.jurisdiction !== state.jurisdiction) {
      throw failure("DO_ID_INVALID", TypeError);
    }
    return makeId(
      state.prefix + hex(idPayload(state.key, randomBytes(ID_BODY_BYTES), jurisdiction)),
      undefined,
      jurisdiction,
    );
  }

  idFromString(value: string) {
    const state = namespaceState.get(this)!;
    const decoded = decodeId(state, value);
    return makeId(decoded.value, undefined, decoded.jurisdiction);
  }

  get(id: DurableObjectId, options?: unknown) {
    getOptions(options);
    if (!idState.has(id)) throw failure("DO_ID_INVALID", TypeError);
    const state = namespaceState.get(this)!;
    const identity = idState.get(id)!;
    if (!identity.value.startsWith(state.prefix)) throw failure("DO_ID_INVALID", TypeError);
    if (state.jurisdiction !== undefined && identity.jurisdiction !== state.jurisdiction) {
      throw failure("DO_ID_INVALID", TypeError);
    }
    return stubProxy(id, state.raw);
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
  return "startRpc" in raw && typeof raw.startRpc === "function"
    && "cancelOrder" in raw && typeof raw.cancelOrder === "function"
    && "prepareConnect" in raw && typeof raw.prepareConnect === "function"
    && "cancelConnect" in raw && typeof raw.cancelConnect === "function"
    && "fetch" in raw && typeof raw.fetch === "function"
    && "connect" in raw && typeof raw.connect === "function";
}

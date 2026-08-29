import { env as currentEnv, RpcTarget, waitUntil } from "cloudflare:workers";
import {
  childServiceFrame, currentServiceFrame, withServiceScope, type ServiceFrame,
} from "./scope.js";

interface ServiceRequestWire {
  readonly url: string;
  readonly method: string;
  readonly headers: readonly (readonly [string, string])[];
  readonly body: ReadableStream<Uint8Array> | null;
}

interface NativeServiceTransport {
  fetchService(frame: ServiceFrame, request: ServiceRequestWire): unknown;
  rpc(frame: ServiceFrame, method: string, args: unknown[]): unknown;
  get(frame: ServiceFrame, property: string): unknown;
  completeRoot(scopeId: string): unknown;
  beginCapability(retention: string, frame: ServiceFrame): unknown;
  releaseRetention(retention: string): unknown;
  completeOperation(handle: string): unknown;
  retainCapability(handle: string, owner: "caller" | "target"): unknown;
}

interface NativeCapabilityHandle extends Disposable {
  call(frame: ServiceFrame, operation: "call" | "get", method: string, args: unknown[]): unknown;
  dup(): NativeCapabilityHandle;
  releaseCapability(): unknown;
}

interface CapabilityEnvelope {
  readonly __openComputeServiceCapability: 1;
  readonly kind: "function" | "target";
  readonly handle: NativeCapabilityHandle;
}

interface CapabilityAdmission {
  readonly handle: string;
  readonly frame: string;
  readonly deadlineMs: number;
}

interface CapabilityController {
  beginCapability(retention: string, frame: ServiceFrame): unknown;
  releaseRetention(retention: string): unknown;
  completeOperation(handle: string): unknown;
  retainCapability(handle: string, owner: "caller" | "target"): unknown;
  dup?(): CapabilityController;
  [Symbol.dispose]?(): void;
}

interface RetentionController extends Disposable {
  begin(frame: ServiceFrame): unknown;
  complete(handle: string): unknown;
  release(): unknown;
  dup(): RetentionController;
}

const METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const RESERVED = new Set([
  "constructor", "prototype", "__proto__", "then", "dup",
  "__openComputeServiceRpc", "__openComputeServiceFetch",
]);

function object(value: unknown): value is object {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

function callable(value: unknown): value is (...args: unknown[]) => unknown {
  return typeof value === "function";
}

function failure(code: string): Error {
  const error = Object.assign(new Error(code), { stableCode: code });
  error.stack = `Error: ${code}`;
  return error;
}

function capabilityEnvelope(value: unknown): value is CapabilityEnvelope {
  return object(value)
    && Reflect.get(value, "__openComputeServiceCapability") === 1
    && ["function", "target"].includes(String(Reflect.get(value, "kind")))
    && object(Reflect.get(value, "handle"));
}

function admission(value: unknown): value is CapabilityAdmission {
  return object(value) && typeof Reflect.get(value, "handle") === "string"
    && typeof Reflect.get(value, "frame") === "string"
    && Number.isSafeInteger(Reflect.get(value, "deadlineMs"));
}

function capabilityDeadline<T>(
  promise: Promise<T>,
  deadlineMs: number,
  timedOut: () => void,
): Promise<T> {
  if (!Number.isSafeInteger(deadlineMs) || deadlineMs < 1 || deadlineMs > 30_000) {
    throw failure("SERVICE_UNAVAILABLE");
  }
  return Promise.race([
    promise,
    scheduler.wait(deadlineMs).then(() => {
      timedOut();
      throw failure("SERVICE_TIMEOUT");
    }),
  ]);
}

class SourceCapability extends RpcTarget {
  readonly #target: object;
  readonly #controller: CapabilityController;
  readonly #controllerOwned: boolean;
  readonly #environment: Record<string, unknown>;
  #retention: RetentionController | undefined;
  #retentionOwned = false;
  #disposed = false;

  constructor(
    target: object,
    controller: CapabilityController,
    environment: Record<string, unknown>,
  ) {
    super();
    this.#target = target;
    const duplicate = Reflect.get(controller, "dup");
    this.#controllerOwned = callable(duplicate);
    this.#controller = this.#controllerOwned
      ? Reflect.apply(duplicate as (...args: unknown[]) => CapabilityController, controller, [])
      : controller;
    this.#environment = environment;
  }

  activate(retention: unknown): void {
    if (!object(retention) || !callable(Reflect.get(retention, "begin"))
        || !callable(Reflect.get(retention, "complete"))
        || !callable(Reflect.get(retention, "release"))
        || this.#retention !== undefined) {
      throw failure("SERVICE_BINDING_DENIED");
    }
    const duplicate = Reflect.get(retention, "dup");
    this.#retentionOwned = callable(duplicate);
    this.#retention = this.#retentionOwned
      ? Reflect.apply(duplicate as (...args: unknown[]) => RetentionController, retention, [])
      : retention as RetentionController;
  }

  async call(
    frame: ServiceFrame,
    operation: "call" | "get",
    method: string,
    rawArgs: unknown[],
  ): Promise<unknown> {
    const retention = this.#retention;
    if (!retention) throw failure("SERVICE_BINDING_DENIED");
    const admitted: unknown = await retention.begin(frame);
    if (!admission(admitted)) throw failure("SERVICE_UNAVAILABLE");
    let timedOut = false;
    const execution = (async () => {
      await activateNestedCapabilities(rawArgs, admitted.handle, this.#controller, "caller");
      const encoded = await withServiceScope(
        this.#environment,
        childServiceFrame(frame.scopeId, admitted.frame),
        async () => {
          let value: unknown;
          if (operation === "get") {
            value = Reflect.get(this.#target, method, this.#target);
          } else {
            const args = decodeServiceValue(rawArgs, new WeakMap(), this.#controller);
            if (!Array.isArray(args)) throw failure("SERVICE_BINDING_DENIED");
            if (method === "__call" && callable(this.#target)) {
              value = Reflect.apply(this.#target, undefined, args);
            } else {
              const fn = Reflect.get(this.#target, method, this.#target);
              if (!callable(fn)) throw failure("SERVICE_ENTRYPOINT_NOT_FOUND");
              value = Reflect.apply(fn, this.#target, args);
            }
          }
          return encodeServiceValue(await value, this.#controller);
        },
      );
      await activateNestedCapabilities(encoded, admitted.handle, this.#controller, "target");
      return encoded;
    })();
    try {
      return await capabilityDeadline(execution, admitted.deadlineMs, () => { timedOut = true; });
    } finally {
      if (timedOut) {
        waitUntil(execution.then(
          () => retention.complete(admitted.handle),
          () => retention.complete(admitted.handle),
        ));
      } else {
        await retention.complete(admitted.handle);
      }
    }
  }

  async releaseCapability(): Promise<void> {
    await this.#release();
  }

  async #release(): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    const retention = this.#retention;
    this.#retention = undefined;
    const retentionOwned = this.#retentionOwned;
    this.#retentionOwned = false;
    if (retention) {
      try { await retention.release(); }
      finally { if (retentionOwned) retention[Symbol.dispose](); }
    }
    if (this.#controllerOwned) this.#controller[Symbol.dispose]?.();
    const disposeTarget = Reflect.get(this.#target, Symbol.dispose);
    if (callable(disposeTarget)) Reflect.apply(disposeTarget, this.#target, []);
  }

  [Symbol.dispose](): void {
    waitUntil(this.#release());
  }
}

async function activateNestedCapabilities(
  value: unknown,
  operationHandle: string,
  controller: CapabilityController,
  owner: "caller" | "target",
  seen = new WeakSet<object>(),
): Promise<void> {
  if (!object(value) || seen.has(value)) return;
  seen.add(value);
  if (capabilityEnvelope(value)) {
    const retention: unknown = await controller.retainCapability(operationHandle, owner);
    const activate = Reflect.get(value.handle, "activate");
    if (!callable(activate)) throw failure("SERVICE_BINDING_DENIED");
    await Reflect.apply(activate, value.handle, [retention]);
    return;
  }
  if (!clonableObject(value)) return;
  for (const item of Object.values(value)) {
    await activateNestedCapabilities(item, operationHandle, controller, owner, seen);
  }
}

function clonableObject(value: object): boolean {
  return Array.isArray(value)
    || Object.getPrototypeOf(value) === Object.prototype
    || Object.getPrototypeOf(value) === null;
}

/** Replace local RpcTarget values with generic native capabilities before an RPC hop. */
export function encodeServiceValue(
  value: unknown,
  controller: CapabilityController,
  seen = new WeakMap<object, unknown>(),
): unknown {
  if (value instanceof RpcTarget || typeof value === "function") {
    if (!object(currentEnv)) throw failure("SERVICE_BINDING_DENIED");
    return Object.freeze({
      __openComputeServiceCapability: 1,
      kind: typeof value === "function" ? "function" : "target",
      handle: new SourceCapability(value, controller, currentEnv as Record<string, unknown>),
    });
  }
  if (!object(value) || !clonableObject(value)) return value;
  const prior = seen.get(value);
  if (prior !== undefined) return prior;
  if (Array.isArray(value)) {
    const output: unknown[] = [];
    seen.set(value, output);
    for (const item of value) output.push(encodeServiceValue(item, controller, seen));
    return output;
  }
  const output: Record<string, unknown> = Object.create(Object.getPrototypeOf(value));
  seen.set(value, output);
  for (const [key, item] of Object.entries(value)) {
    Object.defineProperty(output, key, {
      value: encodeServiceValue(item, controller, seen), enumerable: true, writable: true, configurable: true,
    });
  }
  return output;
}

function serviceMember(
  call: (operation: "call" | "get", args: unknown[]) => unknown,
  callbackController?: CapabilityController,
): (...args: unknown[]) => unknown {
  const member = (...args: unknown[]) => result(call("call", args), callbackController);
  return new Proxy(member, {
    get(target, property, receiver) {
      if (property === "then") {
        return (resolved: (value: unknown) => unknown, rejected?: (reason: unknown) => unknown) =>
          Promise.resolve(result(call("get", []), callbackController)).then(resolved, rejected);
      }
      if (typeof property === "string" && RESERVED.has(property)) {
        throw failure("SERVICE_BINDING_DENIED");
      }
      const value: unknown = Reflect.get(target, property, receiver);
      return callable(value) ? value.bind(target) : value;
    },
  });
}

function result(raw: unknown, callbackController?: CapabilityController): unknown {
  if (!object(raw)) return raw;
  const then = Reflect.get(raw, "then");
  if (!callable(then)) return decodeServiceValue(raw, new WeakMap(), callbackController);
  return new Proxy(raw, {
    get(target, property, receiver) {
      if (property === "then") {
        return (resolved: (value: unknown) => unknown, rejected?: (reason: unknown) => unknown) =>
          Reflect.apply(then, target, [
            (value: unknown) => resolved(decodeServiceValue(value, new WeakMap(), callbackController)),
            rejected,
          ]);
      }
      if (typeof property !== "string" || RESERVED.has(property) || !METHOD.test(property)) {
        return Reflect.get(target, property, receiver);
      }
      return serviceMember((operation, args) => {
        const envelope = Reflect.get(target, "handle");
        if (!object(envelope)) throw failure("SERVICE_UNAVAILABLE");
        const call = Reflect.get(envelope, "call");
        if (!callable(call)) throw failure("SERVICE_UNAVAILABLE");
        return Reflect.apply(call, envelope, [
          currentServiceFrame(), operation, property,
          callbackController ? encodeServiceValue(args, callbackController) as unknown[] : args,
        ]);
      }, callbackController);
    },
  });
}

interface CapabilityGroup {
  remaining: number;
  released: boolean;
}

function duplicateCapability(
  handle: NativeCapabilityHandle,
  kind: "function" | "target",
  group: CapabilityGroup,
  callbackController?: CapabilityController,
): object {
  group.remaining += 1;
  return capability(handle.dup(), kind, group, callbackController);
}

function disposeCapability(handle: NativeCapabilityHandle, group: CapabilityGroup): void {
  if (group.released || group.remaining < 1) return;
  group.remaining -= 1;
  if (group.remaining > 0) {
    handle[Symbol.dispose]();
    return;
  }
  group.released = true;
  const release = Reflect.get(handle, "releaseCapability");
  if (!callable(release)) {
    handle[Symbol.dispose]();
    return;
  }
  waitUntil(Promise.resolve(Reflect.apply(release, handle, [])).then(
    () => handle[Symbol.dispose](),
    () => handle[Symbol.dispose](),
  ));
}

function capability(
  handle: NativeCapabilityHandle,
  kind: "function" | "target",
  group: CapabilityGroup = { remaining: 1, released: false },
  callbackController?: CapabilityController,
): object {
  if (kind === "function") {
    const callback = (...args: unknown[]) => result(
      handle.call(
        currentServiceFrame(), "call", "__call",
        callbackController ? encodeServiceValue(args, callbackController) as unknown[] : args,
      ),
    );
    return new Proxy(callback, {
      get(target, property, receiver) {
        if (property === "then") return undefined;
        if (property === "dup") return () => duplicateCapability(handle, kind, group, callbackController);
        if (property === Symbol.dispose) return () => disposeCapability(handle, group);
        if (typeof property === "string" && RESERVED.has(property)) {
          throw failure("SERVICE_BINDING_DENIED");
        }
        const value: unknown = Reflect.get(target, property, receiver);
        return callable(value) ? value.bind(target) : value;
      },
    });
  }
  const target = Object.create(null) as object;
  return new Proxy(target, {
    get(_owner, property) {
      if (property === "then") return undefined;
      if (property === "dup") return () => duplicateCapability(handle, kind, group, callbackController);
      if (property === Symbol.dispose) return () => disposeCapability(handle, group);
      if (typeof property !== "string" || RESERVED.has(property) || !METHOD.test(property)) {
        throw failure("SERVICE_BINDING_DENIED");
      }
      return serviceMember((operation, args) => handle.call(
        currentServiceFrame(), operation, property,
        callbackController ? encodeServiceValue(args, callbackController) as unknown[] : args,
      ), callbackController);
    },
  });
}

/** Recursively restore trusted generic capability envelopes as native-RPC-backed facades. */
export function decodeServiceValue(
  value: unknown,
  seen = new WeakMap<object, unknown>(),
  callbackController?: CapabilityController,
): unknown {
  if (capabilityEnvelope(value)) return capability(value.handle, value.kind, undefined, callbackController);
  if (!object(value) || value instanceof Date || value instanceof Request || value instanceof Response
      || value instanceof ReadableStream || value instanceof WritableStream
      || value instanceof ArrayBuffer || ArrayBuffer.isView(value)
      || value instanceof Map || value instanceof Set || value instanceof Error || value instanceof RegExp) {
    return value;
  }
  const prior = seen.get(value);
  if (prior !== undefined) return prior;
  if (Array.isArray(value)) {
    const output: unknown[] = [];
    seen.set(value, output);
    for (const item of value) output.push(decodeServiceValue(item, seen, callbackController));
    return output;
  }
  if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) {
    return value;
  }
  const output: Record<string, unknown> = Object.create(Object.getPrototypeOf(value));
  seen.set(value, output);
  for (const [key, item] of Object.entries(value)) {
    Object.defineProperty(output, key, {
      value: decodeServiceValue(item, seen, callbackController), enumerable: true, writable: true, configurable: true,
    });
  }
  return output;
}

/** Tenant-visible Cloudflare-style Service binding backed by a trusted system transport. */
export class ServiceBinding {
  readonly #transport: NativeServiceTransport;

  constructor(raw: unknown) {
    if (!object(raw) || !callable(Reflect.get(raw, "fetchService"))
        || !callable(Reflect.get(raw, "rpc")) || !callable(Reflect.get(raw, "get"))) {
      throw failure("SERVICE_BINDING_DENIED");
    }
    this.#transport = raw as NativeServiceTransport;
    const proxy = new Proxy(this, {
      get(owner, property, receiver) {
        if (property === "then") return undefined;
        if (typeof property === "string" && RESERVED.has(property)) {
          throw failure("SERVICE_BINDING_DENIED");
        }
        const own = Reflect.get(owner, property, receiver);
        if (own !== undefined || typeof property !== "string") {
          return callable(own) ? own.bind(owner) : own;
        }
        if (!METHOD.test(property)) {
          throw failure("SERVICE_BINDING_DENIED");
        }
        const callbackController = controller(owner.#transport);
        return serviceMember((operation, args) => operation === "get"
          ? owner.#transport.get(currentServiceFrame(), property)
          : owner.#transport.rpc(
            currentServiceFrame(),
            property,
            encodeServiceValue(args, callbackController) as unknown[],
          ), callbackController);
      },
    });
    transports.set(this, this.#transport);
    transports.set(proxy, this.#transport);
    return proxy;
  }

  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const request = new Request(input, init);
    const raw = this.#transport.fetchService(currentServiceFrame(), {
      url: request.url,
      method: request.method,
      headers: [...request.headers],
      body: request.body,
    });
    return Promise.resolve(raw).then(value => {
      if (!(value instanceof Response)) throw failure("SERVICE_UNAVAILABLE");
      return value;
    });
  }
}

function controller(transport: NativeServiceTransport): CapabilityController {
  return {
    beginCapability: (retention, frame) => transport.beginCapability(retention, frame),
    releaseRetention: retention => transport.releaseRetention(retention),
    completeOperation: handle => transport.completeOperation(handle),
    retainCapability: (handle, owner) => transport.retainCapability(handle, owner),
  };
}

const transports = new WeakMap<object, NativeServiceTransport>();

/** Complete every raw controller participating in one drained root event. */
export async function completeServiceScope(
  userEnv: Record<string, unknown>,
  scopeId: string,
): Promise<void> {
  const unique = new Set<NativeServiceTransport>();
  for (const value of Object.values(userEnv)) {
    if (object(value)) {
      const transport = transports.get(value);
      if (transport) unique.add(transport);
    }
  }
  await Promise.allSettled([...unique].map(transport =>
    Promise.resolve(transport.completeRoot(scopeId))));
}

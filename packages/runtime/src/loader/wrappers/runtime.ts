import {
  exports as currentExports, tracing, waitUntil, withEnv, WorkerEntrypoint,
} from "cloudflare:workers";
import {
  completeServiceScope, decodeServiceValue, encodeServiceValue,
} from "../../services/facade.js";
import {
  childServiceFrame, rootServiceFrame, withServiceScope, type ServiceFrame,
} from "../../services/scope.js";

/** Explicit tenant variables and product capabilities, never host service bindings. */
export type Environment = Record<string, unknown>;
export type EnvironmentWrapper = (env: Environment) => Environment;
export interface BindingFactory {
  names: readonly string[];
  create: new(raw: unknown, durableObject: boolean) => object;
}
export interface CacheRuntime {
  readonly context: object;
  dispatch(origin: () => unknown, request: Request, ctx: ExecutionContext): Promise<Response>;
}
export interface CacheRuntimeFactory {
  bind(environment: Environment): CacheRuntime | undefined;
}
interface TenantConstructor {
  new(ctx: unknown, env: Environment): object;
  readonly prototype: object;
}
interface CompletionReporter extends Disposable {
  beginCapability(retention: string, frame: ServiceFrame): unknown;
  releaseRetention(retention: string): unknown;
  completeOperation(handle: string): unknown;
  retainCapability(handle: string, owner: "caller" | "target"): unknown;
  dup(): CompletionReporter;
}
export interface TrackedContext<Context extends object = object> {
  readonly context: Context;
  readonly tasks: Promise<unknown>[];
  readonly extendLifetime: (promise: Promise<unknown>) => void;
}
type Callable = (this: unknown, ...args: unknown[]) => unknown;
const PRIVATE_ALARM_INDEX = "__OPEN_COMPUTE_PRIVATE_ALARM_INDEX";
const PRIVATE_CACHE = "__OPEN_COMPUTE_PRIVATE_CACHE";
const SERVICE_RPC = "__openComputeServiceRpc";
const SERVICE_FETCH = "__openComputeServiceFetch";
const SERVICE_GET = "__openComputeServiceGet";
const PUBLIC_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const RESERVED_METHODS = new Set([
  "constructor", "prototype", "__proto__", "then", SERVICE_RPC, SERVICE_FETCH, SERVICE_GET,
]);
const trackedInstances = new WeakMap<object, TrackedContext>();
const instanceEnvironments = new WeakMap<object, Environment>();
const instanceCaches = new WeakMap<object, CacheRuntime>();

function callable(value: unknown): value is Callable { return typeof value === "function"; }

/** Validate only constructibility; tenant code still runs inside its isolate. */
export function tenantConstructor(value: unknown): TenantConstructor {
  if (!constructible(value)) throw new Error("missing entrypoint");
  // A scoped base keeps `super()` as a normal, type-checked constructor call.
  // Reflect.construct preserves new.target and the native inheritance chain.
  return new Proxy(value, {
    construct(target, args: unknown[], newTarget) {
      const env = args[1];
      if (env === null || typeof env !== "object" || Array.isArray(env)) throw new Error("invalid tenant env");
      const instance: unknown = withEnv(env, () => Reflect.construct(target, args, newTarget));
      if (instance === null || (typeof instance !== "object" && typeof instance !== "function")) {
        throw new Error("invalid tenant constructor result");
      }
      return instance;
    },
  });
}

function constructible(value: unknown): value is TenantConstructor {
  if (typeof value !== "function") return false;
  const prototype: unknown = Reflect.get(value, "prototype");
  if (prototype === null || typeof prototype !== "object") return false;
  try { Reflect.construct(Object, [], value); return true; }
  catch { return false; }
}

/** Wrap each declared capability once and remove the private alarm capability. */
export function createEnvironment(factories: readonly BindingFactory[], durableObject: boolean): EnvironmentWrapper {
  const wrapped = new WeakSet<object>();
  return env => {
    if (wrapped.has(env)) return env;
    const out: Environment = {};
    for (const [key, value] of Object.entries(env)) {
      if (key !== PRIVATE_ALARM_INDEX && key !== PRIVATE_CACHE) Object.defineProperty(out, key, {
        value, enumerable: true, configurable: true, writable: true,
      });
    }
    for (const factory of factories) {
      for (const name of factory.names) out[name] = new factory.create(out[name], durableObject);
    }
    wrapped.add(out);
    return out;
  };
}

/** Track waitUntil work while preserving the native execution-context receiver. */
export function trackExecutionContext<Context extends object>(
  ctx: Context,
  cacheContext?: object,
): TrackedContext<Context> {
  const tasks: Promise<unknown>[] = [];
  const nativeWaitUntil: unknown = Reflect.get(ctx, "waitUntil", ctx);
  const extendLifetime = callable(nativeWaitUntil)
    ? (promise: Promise<unknown>) => { Reflect.apply(nativeWaitUntil, ctx, [promise]); }
    : (promise: Promise<unknown>) => { waitUntil(promise); };
  const context = new Proxy(ctx, {
    get(target, property) {
      if (property === "cache" && cacheContext !== undefined) return cacheContext;
      if (property === "waitUntil") return (promise: Promise<unknown>) => {
        const tracked = Promise.resolve(promise);
        tasks.push(tracked);
        extendLifetime(tracked);
      };
      const value: unknown = Reflect.get(target, property, target);
      return callable(value) ? value.bind(target) : value;
    },
  });
  return { context, tasks, extendLifetime };
}

function invoke(
  owner: unknown,
  fn: Callable,
  args: unknown[],
  env: Environment,
  trackedOverride?: TrackedContext,
): unknown {
  const frame = rootServiceFrame();
  const tracked = trackedOverride ?? (owner !== null && typeof owner === "object"
    ? trackedInstances.get(owner) : undefined);
  try {
    const value = withServiceScope(env, frame, scoped =>
      withEnv(scoped, () => Reflect.apply(fn, owner, args)));
    return rootResult(value, env, frame.scopeId, tracked);
  } catch (error) {
    scheduleRootCompletion(env, frame.scopeId, tracked);
    throw error;
  }
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve: () => void = () => {};
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

function syntheticContext(): ExecutionContext {
  return {
    waitUntil,
    passThroughOnException() {},
    exports: currentExports,
    props: undefined,
    async restore() { throw new Error("SERVICE_BINDING_DENIED"); },
    mapVirtualHost() { throw new Error("SERVICE_BINDING_DENIED"); },
    tracing,
    abort() { throw new Error("SERVICE_BINDING_DENIED"); },
  };
}

function wrapRootStream(stream: ReadableStream<Uint8Array>, done: () => void): ReadableStream<Uint8Array> {
  const reader = stream.getReader();
  let finished = false;
  const finish = () => { if (!finished) { finished = true; done(); } };
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const part = await reader.read();
        if (part.done) { finish(); controller.close(); }
        else controller.enqueue(part.value);
      } catch (error) { finish(); controller.error(error); }
    },
    async cancel(reason) { try { await reader.cancel(reason); } finally { finish(); } },
  });
}

function wrapRootWritable(stream: WritableStream<unknown>, done: () => void): WritableStream<unknown> {
  const writer = stream.getWriter();
  let finished = false;
  const finish = () => { if (!finished) { finished = true; done(); } };
  writer.closed.then(finish, finish);
  return new WritableStream<unknown>({
    write(chunk) { return writer.write(chunk); },
    async close() { try { await writer.close(); } finally { finish(); } },
    async abort(reason) { try { await writer.abort(reason); } finally { finish(); } },
  });
}

function resultDrain(value: unknown): { value: unknown; drained: Promise<void> } {
  if (value instanceof Response) {
    if (value.webSocket) {
      const drained = new Promise<void>(resolve => {
        value.webSocket!.addEventListener("close", () => resolve(), { once: true });
        value.webSocket!.addEventListener("error", () => resolve(), { once: true });
      });
      return { value, drained };
    }
    if (!value.body) return { value, drained: Promise.resolve() };
    const end = deferred();
    return {
      value: new Response(wrapRootStream(value.body, end.resolve), {
        status: value.status, statusText: value.statusText, headers: value.headers,
      }),
      drained: end.promise,
    };
  }
  if (value instanceof ReadableStream) {
    const end = deferred();
    return { value: wrapRootStream(value, end.resolve), drained: end.promise };
  }
  if (value instanceof WritableStream) {
    const end = deferred();
    return { value: wrapRootWritable(value, end.resolve), drained: end.promise };
  }
  if (value instanceof Request) {
    if (!value.body) return { value, drained: Promise.resolve() };
    const end = deferred();
    return {
      value: new Request(value, { body: wrapRootStream(value.body, end.resolve) }),
      drained: end.promise,
    };
  }
  return { value, drained: Promise.resolve() };
}

function scheduleRootCompletion(
  env: Environment,
  scopeId: string,
  tracked?: TrackedContext,
  drained: Promise<void> = Promise.resolve(),
): void {
  const background = tracked ? drainTrackedTasks(tracked) : Promise.resolve();
  const completion = Promise.all([background, drained])
    .then(() => completeServiceScope(env, scopeId))
    .catch(() => undefined);
  if (tracked) tracked.extendLifetime(completion);
}

async function drainTrackedTasks(tracked: TrackedContext): Promise<void> {
  let consumed = 0;
  while (consumed < tracked.tasks.length) {
    const pending = tracked.tasks.slice(consumed);
    consumed = tracked.tasks.length;
    await Promise.allSettled(pending);
  }
}

function rootResult(
  raw: unknown,
  env: Environment,
  scopeId: string,
  tracked?: TrackedContext,
): unknown {
  if (raw instanceof Promise) {
    return raw.then(value => {
      const result = resultDrain(value);
      scheduleRootCompletion(env, scopeId, tracked, result.drained);
      return result.value;
    }, error => {
      scheduleRootCompletion(env, scopeId, tracked);
      throw error;
    });
  }
  const result = resultDrain(raw);
  scheduleRootCompletion(env, scopeId, tracked, result.drained);
  return result.value;
}

function backgroundSignal(tracked: TrackedContext): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    async start(controller) {
      await drainTrackedTasks(tracked);
      controller.close();
    },
  });
}

function serviceSuccess(value: unknown, tracked: TrackedContext): unknown {
  return Object.freeze({ ok: true, value, background: backgroundSignal(tracked) });
}

function serviceFailure(error: unknown, tracked: TrackedContext): unknown {
  return Object.freeze({ ok: false, error, background: backgroundSignal(tracked) });
}

function serviceMethod(owner: object, method: string): Callable {
  if (!PUBLIC_METHOD.test(method) || RESERVED_METHODS.has(method)) throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
  const value: unknown = Reflect.get(owner, method, owner);
  if (!callable(value)) throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
  return value;
}

async function invokeService(
  owner: object,
  method: string,
  rawArgs: unknown[],
  env: Environment,
  frame: ServiceFrame,
  reporter: CompletionReporter,
  tracked: TrackedContext,
): Promise<unknown> {
  try {
    const args = decodeServiceValue(rawArgs, new WeakMap(), reporter);
    if (!Array.isArray(args)) throw new Error("SERVICE_BINDING_DENIED");
    const value = await withServiceScope(env, frame, scoped => withEnv(scoped, async () => {
      const value = await Reflect.apply(serviceMethod(owner, method), owner, args);
      return encodeServiceValue(value, reporter);
    }));
    return serviceSuccess(value, tracked);
  } catch (error) {
    return serviceFailure(error, tracked);
  }
}

async function getService(
  owner: object,
  property: string,
  env: Environment,
  frame: ServiceFrame,
  reporter: CompletionReporter,
  tracked: TrackedContext,
): Promise<unknown> {
  if (!PUBLIC_METHOD.test(property) || RESERVED_METHODS.has(property)) {
    throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
  }
  try {
    const value = await withServiceScope(env, frame, scoped => withEnv(scoped, async () => {
      const value = await Reflect.get(owner, property, owner);
      return encodeServiceValue(value, reporter);
    }));
    return serviceSuccess(value, tracked);
  } catch (error) {
    return serviceFailure(error, tracked);
  }
}

async function fetchService(
  owner: unknown,
  fn: Callable,
  request: Request,
  env: Environment,
  frame: ServiceFrame,
  _reporter: CompletionReporter,
  tracked: TrackedContext,
  objectHandler: boolean,
  cache?: CacheRuntime,
): Promise<unknown> {
  try {
    const invokeOrigin = () => withServiceScope(env, frame, scoped => withEnv(scoped, () =>
      Reflect.apply(fn, owner, objectHandler ? [request, scoped, tracked.context] : [request])));
    const value = await (cache === undefined
      ? invokeOrigin()
      : cache.dispatch(invokeOrigin, request, tracked.context as ExecutionContext));
    return serviceSuccess(value, tracked);
  } catch (error) {
    return serviceFailure(error, tracked);
  }
}

/** Preserve native/private-field receivers while restoring the tenant env scope. */
export function wrapInstance<T extends object>(
  instance: T,
  env: Environment,
  tracked?: TrackedContext,
  cache?: CacheRuntime,
): T {
  if (tracked) {
    trackedInstances.set(instance, tracked);
    instanceEnvironments.set(instance, env);
  }
  return new Proxy(instance, {
    get(target, property) {
      const value: unknown = Reflect.get(target, property, target);
      if (!callable(value)) return value;
      return (...args: unknown[]) => {
        if (property === "fetch" && cache !== undefined && args[0] instanceof Request && tracked) {
          const operation: Callable = () => cache.dispatch(
            () => Reflect.apply(value, target, args),
            args[0] as Request,
            tracked.context as ExecutionContext,
          );
          return invoke(target, operation, [], env, tracked);
        }
        return invoke(target, value, args, env, tracked);
      };
    },
  });
}

/** Invoke a system-owned entrypoint adapter with the same root Service lifecycle as public events. */
export function invokeEntrypoint(
  owner: unknown,
  fn: Callable,
  args: unknown[],
  env: Environment,
  tracked: TrackedContext,
): unknown {
  return invoke(owner, fn, args, env, tracked);
}

function normalizedEvent(kind: string, event: unknown): unknown {
  if (kind !== "scheduled" || event === null || typeof event !== "object"
      || Reflect.get(event, "type") !== undefined) return event;
  return new Proxy(event, {
    get(target, property) {
      if (property === "type") return "scheduled";
      const value: unknown = Reflect.get(target, property, target);
      return callable(value) ? value.bind(target) : value;
    },
  });
}

function wrapHandler(owner: unknown, fn: Callable, kind: string, wrapEnv: EnvironmentWrapper,
  cache?: CacheRuntimeFactory) {
  return (event: unknown, env: Environment, ctx: ExecutionContext): unknown => {
    const boundCache = cache?.bind(env);
    const wrapped = wrapEnv(env);
    const tracked = trackExecutionContext(ctx, boundCache?.context);
    const args = [normalizedEvent(kind, event), wrapped, tracked.context];
    if (kind === "fetch" && boundCache !== undefined && event instanceof Request) {
      const operation: Callable = () => boundCache.dispatch(
        () => Reflect.apply(fn, owner, args), event, tracked.context as ExecutionContext,
      );
      return invoke(owner, operation, [], wrapped, tracked);
    }
    return invoke(owner, fn, args, wrapped, tracked);
  };
}

/** Wrap class entrypoints without replacing their native inheritance chain. */
export function wrapEntrypoint(target: unknown, wrapEnv: EnvironmentWrapper, name?: string,
  cache?: CacheRuntimeFactory): TenantConstructor {
  const Base = tenantConstructor(target);
  const Wrapped = class extends Base {
    constructor(ctx: unknown, env: Environment) {
      const boundCache = cache?.bind(env);
      const wrapped = wrapEnv(env);
      if (ctx === null || typeof ctx !== "object") throw new Error("invalid execution context");
      const tracked = trackExecutionContext(ctx as ExecutionContext, boundCache?.context);
      super(tracked.context, wrapped);
      trackedInstances.set(this, tracked);
      instanceEnvironments.set(this, wrapped);
      if (boundCache !== undefined) instanceCaches.set(this, boundCache);
      return wrapInstance(this, wrapped, tracked, boundCache);
    }

    [SERVICE_RPC](scopeId: string, frame: string, reporter: CompletionReporter, method: string, args: unknown[]) {
      const tracked = trackedInstances.get(this);
      if (!tracked) throw new Error("SERVICE_BINDING_DENIED");
      const environment = instanceEnvironments.get(this);
      if (!environment) throw new Error("SERVICE_BINDING_DENIED");
      return invokeService(this, method, args, environment,
        childServiceFrame(scopeId, frame), reporter, tracked);
    }

    [SERVICE_GET](scopeId: string, frame: string, reporter: CompletionReporter, property: string) {
      const tracked = trackedInstances.get(this);
      if (!tracked) throw new Error("SERVICE_BINDING_DENIED");
      const environment = instanceEnvironments.get(this);
      if (!environment) throw new Error("SERVICE_BINDING_DENIED");
      return getService(this, property, environment,
        childServiceFrame(scopeId, frame), reporter, tracked);
    }

    [SERVICE_FETCH](scopeId: string, frame: string, reporter: CompletionReporter, request: Request) {
      const tracked = trackedInstances.get(this);
      if (!tracked) throw new Error("SERVICE_BINDING_DENIED");
      const environment = instanceEnvironments.get(this);
      if (!environment) throw new Error("SERVICE_BINDING_DENIED");
      try {
      return fetchService(this, serviceMethod(this, "fetch"), request, environment,
          childServiceFrame(scopeId, frame), reporter, tracked, false, instanceCaches.get(this));
      } catch (error) {
        return serviceFailure(error, tracked);
      }
    }
  };
  if (name !== undefined) Object.defineProperty(Wrapped, "name", { value: name });
  return Wrapped;
}

/** Give object/function-style defaults an env-aware private Service fetch entrypoint. */
export function wrapDefaultService(raw: unknown, wrapEnv: EnvironmentWrapper,
  cache?: CacheRuntimeFactory): TenantConstructor {
  if (callable(raw) && /^\s*class\b/.test(Function.prototype.toString.call(raw))) {
    return wrapEntrypoint(raw, wrapEnv, "__OpenComputeDefaultService", cache);
  }
  const owner = raw !== null && typeof raw === "object" ? raw : undefined;
  const fetch = owner === undefined ? raw : Reflect.get(owner, "fetch");
  return class OpenComputeDefaultService extends WorkerEntrypoint<Environment> {
    readonly #environment: Environment;
    readonly #tracked: TrackedContext;
    readonly #cache: CacheRuntime | undefined;

    constructor(ctx: unknown, env: Environment) {
      if (ctx === null || typeof ctx !== "object") throw new Error("invalid execution context");
      const boundCache = cache?.bind(env);
      const wrapped = wrapEnv(env);
      const tracked = trackExecutionContext(ctx as ExecutionContext, boundCache?.context);
      super(tracked.context, wrapped);
      this.#cache = boundCache;
      this.#environment = wrapped;
      this.#tracked = tracked;
    }

    [SERVICE_FETCH](
      scopeId: string,
      frame: string,
      reporter: CompletionReporter,
      request: Request,
    ): unknown {
      if (!callable(fetch)) return serviceFailure(new Error("SERVICE_ENTRYPOINT_NOT_FOUND"), this.#tracked);
      return fetchService(owner, fetch, request, this.#environment,
        childServiceFrame(scopeId, frame), reporter, this.#tracked, true, this.#cache);
    }
  };
}

/** Preserve object handlers, function-style fetch, and class-style Workers. */
export function wrapDefault(raw: unknown, wrapEnv: EnvironmentWrapper, cache?: CacheRuntimeFactory): unknown {
  if (raw !== null && typeof raw === "object") {
    const result: Environment = { ...raw };
    for (const key of ["fetch", "scheduled", "queue", "tail"]) {
      const handler: unknown = Reflect.get(raw, key);
      if (callable(handler)) result[key] = wrapHandler(raw, handler, key, wrapEnv, cache);
    }
    const fetch: unknown = Reflect.get(raw, "fetch");
    if (callable(fetch)) result[SERVICE_FETCH] = (
      scopeId: string, frame: string, reporter: CompletionReporter, request: Request,
    ) => {
      const tracked = trackExecutionContext(syntheticContext());
      return fetchService(raw, fetch, request, result, childServiceFrame(scopeId, frame), reporter,
        tracked, true);
    };
    return result;
  }
  if (callable(raw)) {
    return /^\s*class\b/.test(Function.prototype.toString.call(raw))
      ? wrapEntrypoint(raw, wrapEnv, undefined, cache)
      : { fetch: wrapHandler(undefined, raw, "fetch", wrapEnv, cache) };
  }
  return raw;
}

/** Validation checks the actual module namespace before returning its probe handler. */
export function validationHandler(tenant: Environment, name: string) {
  if (!(name in tenant)) throw new Error("missing entrypoint");
  return { fetch(): Response { return new Response("open-compute-validation-v1"); } };
}

import {
  exports as currentExports,
  waitUntil,
  withEnv,
  withExports,
  WorkerEntrypoint,
} from "cloudflare:workers";
import type { CacheRuntime, CacheRuntimeFactory } from "../../cache/facade.js";
import {
  decodeServiceValue,
  encodeServiceValue,
} from "../../services/facade.js";
import {
  childServiceFrame,
  rootServiceFrame,
  withServiceScope,
  type ServiceFrame,
} from "../../services/scope.js";
import {
  drainTrackedTasks,
  resultDrain,
  rootResult,
  scheduleRootCompletion,
  serviceFailure,
  serviceSuccess,
} from "./completion.js";
import { tenantExports } from "./loopback.js";
import type {
  BindingFactory,
  Callable,
  CompletionReporter,
  Environment,
  EnvironmentWrapper,
  TenantConstructor,
  TrackedContext,
} from "./types.js";

export { loopbackDurableObjectMetadata } from "./loopback.js";
export type {
  BindingFactory,
  Environment,
  EnvironmentWrapper,
  TenantConstructor,
  TrackedContext,
} from "./types.js";

type WorkflowScheduleTrigger = (
  value: unknown,
  schedule: { cron: string; scheduledTime: number },
) => Promise<void>;
interface ScheduledWorkflowRuntime {
  readonly targets: readonly {
    cron: string;
    scheduledHandler: boolean;
    workflowBindings: readonly string[];
  }[];
  readonly trigger: WorkflowScheduleTrigger;
}
const PRIVATE_ALARM_INDEX = "__OPEN_COMPUTE_PRIVATE_ALARM_INDEX";
const PRIVATE_FACET_MANAGER = "__OPEN_COMPUTE_PRIVATE_FACET_MANAGER";
const PRIVATE_FACET_AUTHORITY = "__OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY";
const PRIVATE_FACET_PATH = "__OPEN_COMPUTE_PRIVATE_FACET_PATH";
const PRIVATE_FACET_PROPS = "__OPEN_COMPUTE_PRIVATE_FACET_PROPS";
const PRIVATE_NATIVE_FACETS = "__OPEN_COMPUTE_PRIVATE_NATIVE_FACETS";
const PRIVATE_CACHE = "__OPEN_COMPUTE_PRIVATE_CACHE";
const SERVICE_RPC = "__openComputeServiceRpc";
const SERVICE_GET = "__openComputeServiceGet";
const PUBLIC_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const SCHEDULED_WORKFLOW_BINDING = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;
const RESERVED_METHODS = new Set([
  "constructor",
  "prototype",
  "__proto__",
  "then",
  SERVICE_RPC,
  SERVICE_GET,
]);
interface NativeFetchContext {
  scopeId: string;
  frame: string;
  completion: Fetcher;
}
const nativeFetchContexts = new WeakMap<object, NativeFetchContext>();

function serviceFetchContext(ctx: object): {
  context: object;
  native?: NativeFetchContext;
} {
  const props: unknown = Reflect.get(ctx, "props", ctx);
  if (props === null || typeof props !== "object") return { context: ctx };
  const native: unknown = Reflect.get(props, "__OPEN_COMPUTE_SERVICE_FETCH");
  if (native === null || typeof native !== "object") return { context: ctx };
  const completion: unknown = Reflect.get(native, "completion");
  if (
    completion === null ||
    typeof completion !== "object" ||
    !callable(Reflect.get(completion, "fetch"))
  )
    return { context: ctx };
  if (
    typeof Reflect.get(native, "scopeId") !== "string" ||
    typeof Reflect.get(native, "frame") !== "string"
  )
    throw new Error("SERVICE_BINDING_DENIED");
  return {
    context: new Proxy(ctx, {
      get(target, property) {
        if (property === "props") return Reflect.get(props, "userProps");
        const value: unknown = Reflect.get(target, property, target);
        return callable(value) ? value.bind(target) : value;
      },
    }),
    native: {
      scopeId: Reflect.get(native, "scopeId"),
      frame: Reflect.get(native, "frame"),
      completion: completion as Fetcher,
    },
  };
}

async function nativeServiceFetch(
  owner: unknown,
  fn: Callable,
  request: Request,
  env: Environment,
  tracked: TrackedContext,
  native: NativeFetchContext,
  objectHandler: boolean,
  cache?: CacheRuntime,
): Promise<Response> {
  let drained: Promise<void> = Promise.resolve();
  let handoffWebSocket = false;
  try {
    const invokeOrigin = () =>
      withServiceScope(
        env,
        childServiceFrame(native.scopeId, native.frame),
        (scoped) =>
          withTenantEnvironment(scoped, () =>
            Reflect.apply(
              fn,
              owner,
              objectHandler ? [request, scoped, tracked.context] : [request],
            ),
          ),
      );
    const value: unknown = await (cache === undefined
      ? invokeOrigin()
      : cache.dispatch(
          invokeOrigin,
          request,
          tracked.context as ExecutionContext,
        ));
    if (!(value instanceof Response)) throw new Error("SERVICE_UNAVAILABLE");
    const result = resultDrain(value);
    drained = result.drained;
    handoffWebSocket = result.handoffWebSocket;
    return result.value as Response;
  } finally {
    const background = Promise.all([drainTrackedTasks(tracked), drained]);
    tracked.extendLifetime(
      handoffWebSocket
        ? background
        : background.then(async () => {
            const response = await native.completion.fetch(
              "https://service-completion.internal/",
            );
            if (!response.ok) throw new Error("SERVICE_UNAVAILABLE");
          }),
    );
  }
}

const trackedInstances = new WeakMap<object, TrackedContext>();
const instanceEnvironments = new WeakMap<object, Environment>();
function callable(value: unknown): value is Callable {
  return typeof value === "function";
}

/** Read the full native export table before tenant export filtering begins. */
export function trustedContextExports(context: unknown): object | undefined {
  if (context === null || typeof context !== "object") return undefined;
  const value: unknown = Reflect.get(context, "exports", context);
  return value !== null && typeof value === "object" ? value : undefined;
}

function withTenantEnvironment<T>(env: Environment, fn: () => T): T {
  const exports = tenantExports(currentExports);
  return withEnv(env, () => withExports(exports, fn)) as T;
}

/** Validate only constructibility; tenant code still runs inside its isolate. */
export function tenantConstructor(value: unknown): TenantConstructor {
  if (!constructible(value)) throw new Error("missing entrypoint");
  // A scoped base keeps `super()` as a normal, type-checked constructor call.
  // Reflect.construct preserves new.target and the native inheritance chain.
  return new Proxy(value, {
    construct(target, args: unknown[], newTarget) {
      const env = args[1];
      if (env === null || typeof env !== "object" || Array.isArray(env))
        throw new Error("invalid tenant env");
      const instance: unknown = withTenantEnvironment(env as Environment, () =>
        Reflect.construct(target, args, newTarget),
      );
      if (
        instance === null ||
        (typeof instance !== "object" && typeof instance !== "function")
      ) {
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
  try {
    Reflect.construct(Object, [], value);
    return true;
  } catch {
    return false;
  }
}

/** Wrap each declared capability once and remove the private alarm capability. */
export function createEnvironment(
  factories: readonly BindingFactory[],
  durableObject: boolean,
): EnvironmentWrapper {
  const wrapped = new WeakSet<object>();
  return (env) => {
    if (wrapped.has(env)) return env;
    const out: Environment = {};
    for (const [key, value] of Object.entries(env)) {
      if (
        key !== PRIVATE_ALARM_INDEX &&
        key !== PRIVATE_FACET_MANAGER &&
        key !== PRIVATE_FACET_AUTHORITY &&
        key !== PRIVATE_FACET_PATH &&
        key !== PRIVATE_FACET_PROPS &&
        key !== PRIVATE_NATIVE_FACETS &&
        key !== PRIVATE_CACHE
      )
        Object.defineProperty(out, key, {
          value,
          enumerable: true,
          configurable: true,
          writable: true,
        });
    }
    for (const factory of factories) {
      for (const name of factory.names) {
        out[name] = new factory.create(out[name], durableObject, name);
      }
    }
    wrapped.add(out);
    return out;
  };
}

/** Track waitUntil work while preserving the native execution-context receiver. */
export function trackExecutionContext<Context extends object>(
  ctx: Context,
  cacheContext?: object,
  runScope?: <T>(fn: () => T) => T,
  trustedExports?: object,
): TrackedContext<Context> {
  const tasks: Promise<unknown>[] = [];
  const nativeWaitUntil: unknown = Reflect.get(ctx, "waitUntil", ctx);
  const extendLifetime = callable(nativeWaitUntil)
    ? (promise: Promise<unknown>) => {
        Reflect.apply(nativeWaitUntil, ctx, [promise]);
      }
    : (promise: Promise<unknown>) => {
        waitUntil(promise);
      };
  const exports = tenantExports(trustedExports ?? currentExports);
  const context = new Proxy(Object.create(null) as Context, {
    get(_target, property) {
      if (property === "cache" && cacheContext !== undefined)
        return cacheContext;
      if (property === "exports") return exports;
      if (property === "waitUntil")
        return (promise: Promise<unknown>) => {
          const tracked = Promise.resolve(promise);
          tasks.push(tracked);
          extendLifetime(tracked);
        };
      const value: unknown = Reflect.get(ctx, property, ctx);
      return callable(value) ? value.bind(ctx) : value;
    },
    has(_target, property) {
      if (property === "exports") return true;
      if (property === "cache" && cacheContext !== undefined) return true;
      return Reflect.has(ctx, property);
    },
    ownKeys() {
      const keys = Reflect.ownKeys(ctx);
      if (!keys.includes("exports")) keys.push("exports");
      return keys;
    },
    getOwnPropertyDescriptor(_target, property) {
      if (property === "exports") {
        return {
          configurable: true,
          enumerable: true,
          writable: false,
          value: exports,
        };
      }
      if (property === "cache" && cacheContext !== undefined) {
        return {
          configurable: true,
          enumerable: true,
          writable: false,
          value: cacheContext,
        };
      }
      const descriptor = Reflect.getOwnPropertyDescriptor(ctx, property);
      if (!descriptor) return undefined;
      const value: unknown = Reflect.get(ctx, property, ctx);
      return {
        configurable: true,
        enumerable: descriptor.enumerable ?? false,
        writable: descriptor.writable ?? false,
        value: callable(value) ? value.bind(ctx) : value,
      };
    },
    getPrototypeOf() {
      return null;
    },
    set(_target, property, value) {
      if (property === "exports" || property === "cache") return false;
      return Reflect.set(ctx, property, value, ctx);
    },
    defineProperty(_target, property, descriptor) {
      if (property === "exports" || property === "cache") return false;
      return Reflect.defineProperty(ctx, property, descriptor);
    },
    deleteProperty(_target, property) {
      if (property === "exports" || property === "cache") return false;
      return Reflect.deleteProperty(ctx, property);
    },
  });
  return {
    context,
    tasks,
    extendLifetime,
    ...(runScope === undefined ? {} : { runScope }),
  };
}

function invoke(
  owner: unknown,
  fn: Callable,
  args: unknown[],
  env: Environment,
  trackedOverride?: TrackedContext,
): unknown {
  const frame = rootServiceFrame();
  const tracked =
    trackedOverride ??
    (owner !== null && typeof owner === "object"
      ? trackedInstances.get(owner)
      : undefined);
  const run = () => {
    try {
      const value = withServiceScope(env, frame, (scoped) =>
        withTenantEnvironment(scoped, () => Reflect.apply(fn, owner, args)),
      );
      return rootResult(value, env, frame.scopeId, tracked);
    } catch (error) {
      scheduleRootCompletion(env, frame.scopeId, tracked);
      throw error;
    }
  };
  return tracked?.runScope ? tracked.runScope(run) : run();
}

function serviceMethod(owner: object, method: string): Callable {
  if (!PUBLIC_METHOD.test(method) || RESERVED_METHODS.has(method))
    throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
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
    const value = await withServiceScope(env, frame, (scoped) =>
      withTenantEnvironment(scoped, async () => {
        const value = await Reflect.apply(
          serviceMethod(owner, method),
          owner,
          args,
        );
        return encodeServiceValue(value, reporter);
      }),
    );
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
    const value = await withServiceScope(env, frame, (scoped) =>
      withTenantEnvironment(scoped, async () => {
        const value = await Reflect.get(owner, property, owner);
        return encodeServiceValue(value, reporter);
      }),
    );
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
        const native = nativeFetchContexts.get(target);
        if (
          property === "fetch" &&
          native &&
          args[0] instanceof Request &&
          tracked
        ) {
          return nativeServiceFetch(
            target,
            value,
            args[0],
            env,
            tracked,
            native,
            false,
            cache,
          );
        }
        if (
          property === "fetch" &&
          cache !== undefined &&
          args[0] instanceof Request &&
          tracked
        ) {
          const operation: Callable = () =>
            cache.dispatch(
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
  if (
    kind !== "scheduled" ||
    event === null ||
    typeof event !== "object" ||
    Reflect.get(event, "type") !== undefined
  )
    return event;
  return new Proxy(event, {
    get(target, property) {
      if (property === "type") return "scheduled";
      const value: unknown = Reflect.get(target, property, target);
      return callable(value) ? value.bind(target) : value;
    },
  });
}

function scheduledInvocation(
  event: unknown,
  targets: ScheduledWorkflowRuntime["targets"],
): {
  controller: unknown;
  scheduledHandler: boolean;
  workflowBindings: string[];
  schedule: { cron: string; scheduledTime: number };
} {
  if (event === null || typeof event !== "object")
    throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
  const cron: unknown = Reflect.get(event, "cron", event);
  const time: unknown = Reflect.get(event, "scheduledTime", event);
  let scheduledTime = Number.NaN;
  try {
    if (
      typeof time === "number" ||
      (time !== null && typeof time === "object")
    ) {
      scheduledTime = Number(time);
    }
  } catch {
    /* rejected below */
  }
  if (
    typeof cron !== "string" ||
    cron.length < 1 ||
    cron.length > 256 ||
    !Number.isSafeInteger(scheduledTime) ||
    scheduledTime < 0 ||
    scheduledTime % 60_000 !== 0
  ) {
    throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
  }
  const target = targets.find((candidate) => candidate.cron === cron);
  if (
    !target ||
    typeof target.scheduledHandler !== "boolean" ||
    !Array.isArray(target.workflowBindings) ||
    target.workflowBindings.length > 100 ||
    (!target.scheduledHandler && target.workflowBindings.length === 0) ||
    !target.workflowBindings.every(
      (value, index) =>
        typeof value === "string" &&
        SCHEDULED_WORKFLOW_BINDING.test(value) &&
        !value.startsWith("OPEN_COMPUTE_") &&
        !value.startsWith("__") &&
        (index === 0 || target.workflowBindings[index - 1]! < value),
    )
  ) {
    throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
  }
  return {
    controller: normalizedEvent("scheduled", event),
    scheduledHandler: target.scheduledHandler,
    workflowBindings: [...target.workflowBindings],
    schedule: { cron, scheduledTime },
  };
}

async function invokeScheduledWorkflows(
  event: unknown,
  env: Environment,
  runtime: ScheduledWorkflowRuntime,
): Promise<ReturnType<typeof scheduledInvocation>> {
  const invocation = scheduledInvocation(event, runtime.targets);
  await Promise.all(
    invocation.workflowBindings.map((name) =>
      runtime.trigger(env[name], invocation.schedule),
    ),
  );
  return invocation;
}

function wrapHandler(
  owner: unknown,
  fn: Callable,
  kind: string,
  wrapEnv: EnvironmentWrapper,
  cache?: CacheRuntimeFactory,
) {
  return (event: unknown, env: Environment, ctx: ExecutionContext): unknown => {
    const boundCache = cache?.bind(env);
    const trustedExports = trustedContextExports(ctx);
    const wrapped = wrapEnv(env);
    const tracked = trackExecutionContext(
      ctx,
      boundCache?.context,
      undefined,
      trustedExports,
    );
    const args = [normalizedEvent(kind, event), wrapped, tracked.context];
    if (
      kind === "fetch" &&
      boundCache !== undefined &&
      event instanceof Request
    ) {
      const operation: Callable = () =>
        boundCache.dispatch(
          () => Reflect.apply(fn, owner, args),
          event,
          tracked.context as ExecutionContext,
        );
      return invoke(owner, operation, [], wrapped, tracked);
    }
    return invoke(owner, fn, args, wrapped, tracked);
  };
}

/** Wrap class entrypoints without replacing their native inheritance chain. */
export function wrapEntrypoint(
  target: unknown,
  wrapEnv: EnvironmentWrapper,
  name?: string,
  cache?: CacheRuntimeFactory,
  scheduledWorkflows?: ScheduledWorkflowRuntime,
): TenantConstructor {
  const Base = tenantConstructor(target);
  const Wrapped = class extends Base {
    constructor(ctx: unknown, env: Environment) {
      if (ctx === null || typeof ctx !== "object")
        throw new Error("invalid execution context");
      const boundCache = cache?.bind(env);
      const trustedExports = trustedContextExports(ctx);
      const wrapped = wrapEnv(env);
      const service = serviceFetchContext(ctx);
      const tracked = trackExecutionContext(
        service.context as ExecutionContext,
        boundCache?.context,
        undefined,
        trustedExports,
      );
      super(tracked.context, wrapped);
      trackedInstances.set(this, tracked);
      instanceEnvironments.set(this, wrapped);
      if (service.native) nativeFetchContexts.set(this, service.native);
      return wrapInstance(this, wrapped, tracked, boundCache);
    }

    [SERVICE_RPC](
      scopeId: string,
      frame: string,
      reporter: CompletionReporter,
      method: string,
      args: unknown[],
    ) {
      const tracked = trackedInstances.get(this);
      if (!tracked) throw new Error("SERVICE_BINDING_DENIED");
      const environment = instanceEnvironments.get(this);
      if (!environment) throw new Error("SERVICE_BINDING_DENIED");
      return invokeService(
        this,
        method,
        args,
        environment,
        childServiceFrame(scopeId, frame),
        reporter,
        tracked,
      );
    }

    [SERVICE_GET](
      scopeId: string,
      frame: string,
      reporter: CompletionReporter,
      property: string,
    ) {
      const tracked = trackedInstances.get(this);
      if (!tracked) throw new Error("SERVICE_BINDING_DENIED");
      const environment = instanceEnvironments.get(this);
      if (!environment) throw new Error("SERVICE_BINDING_DENIED");
      return getService(
        this,
        property,
        environment,
        childServiceFrame(scopeId, frame),
        reporter,
        tracked,
      );
    }
  };
  if (name !== undefined)
    Object.defineProperty(Wrapped, "name", { value: name });
  if (scheduledWorkflows === undefined) return Wrapped;
  return class extends Wrapped {
    async scheduled(event: unknown): Promise<unknown> {
      const tracked = trackedInstances.get(this);
      const environment = instanceEnvironments.get(this);
      if (!tracked || !environment)
        throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
      const invocation = await invokeScheduledWorkflows(
        event,
        environment,
        scheduledWorkflows,
      );
      if (!invocation.scheduledHandler) return undefined;
      const handler: unknown = Reflect.get(Base.prototype, "scheduled", this);
      if (!callable(handler)) throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
      return invoke(
        this,
        handler,
        [invocation.controller],
        environment,
        tracked,
      );
    }
  };
}

/** Give object/function-style defaults an env-aware private Service fetch entrypoint. */
export function wrapDefaultService(
  raw: unknown,
  wrapEnv: EnvironmentWrapper,
  cache?: CacheRuntimeFactory,
): TenantConstructor {
  if (
    callable(raw) &&
    /^\s*class\b/.test(Function.prototype.toString.call(raw))
  ) {
    return wrapEntrypoint(raw, wrapEnv, "__OpenComputeDefaultService", cache);
  }
  const owner = raw !== null && typeof raw === "object" ? raw : undefined;
  const fetch = owner === undefined ? raw : Reflect.get(owner, "fetch");
  const connectHandler =
    owner === undefined ? undefined : Reflect.get(owner, "connect");
  return class OpenComputeDefaultService extends WorkerEntrypoint<Environment> {
    readonly #environment: Environment;
    readonly #tracked: TrackedContext;
    readonly #cache: CacheRuntime | undefined;

    constructor(ctx: unknown, env: Environment) {
      if (ctx === null || typeof ctx !== "object")
        throw new Error("invalid execution context");
      const boundCache = cache?.bind(env);
      const trustedExports = trustedContextExports(ctx);
      const wrapped = wrapEnv(env);
      const service = serviceFetchContext(ctx);
      const tracked = trackExecutionContext(
        service.context as ExecutionContext,
        boundCache?.context,
        undefined,
        trustedExports,
      );
      super(tracked.context, wrapped);
      if (service.native) nativeFetchContexts.set(this, service.native);
      this.#cache = boundCache;
      this.#environment = wrapped;
      this.#tracked = tracked;
    }

    async fetch(request: Request): Promise<Response> {
      const native = nativeFetchContexts.get(this);
      if (!native || !callable(fetch))
        throw new Error("SERVICE_BINDING_DENIED");
      return nativeServiceFetch(
        owner,
        fetch,
        request,
        this.#environment,
        this.#tracked,
        native,
        true,
        this.#cache,
      );
    }

    async connect(socket: Socket): Promise<void> {
      if (!callable(connectHandler))
        throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
      await invoke(
        owner,
        connectHandler,
        [socket, this.#environment, this.#tracked.context],
        this.#environment,
        this.#tracked,
      );
    }
  };
}

/** Preserve object handlers, function-style fetch, and class-style Workers. */
export function wrapDefault(
  raw: unknown,
  wrapEnv: EnvironmentWrapper,
  cache?: CacheRuntimeFactory,
  scheduledWorkflows?: ScheduledWorkflowRuntime,
): unknown {
  if (raw !== null && typeof raw === "object") {
    const result: Environment = { ...raw };
    for (const key of [
      "fetch",
      "connect",
      "scheduled",
      "queue",
      "email",
      "tail",
      "tailStream",
      "test",
      "trace",
    ]) {
      const handler: unknown = Reflect.get(raw, key);
      if (callable(handler))
        result[key] = wrapHandler(raw, handler, key, wrapEnv, cache);
    }
    if (scheduledWorkflows !== undefined) {
      const scheduled: unknown = Reflect.get(raw, "scheduled");
      result.scheduled = wrapHandler(
        raw,
        async (...args: unknown[]) => {
          const [event, rawEnvironment, rawContext] = args;
          if (
            rawEnvironment === null ||
            typeof rawEnvironment !== "object" ||
            Array.isArray(rawEnvironment) ||
            rawContext === null ||
            typeof rawContext !== "object" ||
            Array.isArray(rawContext)
          ) {
            throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
          }
          const env = rawEnvironment as Environment;
          const ctx = rawContext as ExecutionContext;
          const invocation = await invokeScheduledWorkflows(
            event,
            env,
            scheduledWorkflows,
          );
          if (!invocation.scheduledHandler) return undefined;
          if (!callable(scheduled))
            throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
          return Reflect.apply(scheduled, raw, [
            invocation.controller,
            env,
            ctx,
          ]);
        },
        "scheduled",
        wrapEnv,
        cache,
      );
    }
    return result;
  }
  if (callable(raw)) {
    return /^\s*class\b/.test(Function.prototype.toString.call(raw))
      ? wrapEntrypoint(raw, wrapEnv, undefined, cache, scheduledWorkflows)
      : {
          fetch: wrapHandler(undefined, raw, "fetch", wrapEnv, cache),
          ...(scheduledWorkflows !== undefined
            ? {
                scheduled: wrapHandler(
                  undefined,
                  async (...args: unknown[]) => {
                    const [event, rawEnvironment] = args;
                    if (
                      rawEnvironment === null ||
                      typeof rawEnvironment !== "object" ||
                      Array.isArray(rawEnvironment)
                    ) {
                      throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
                    }
                    const env = rawEnvironment as Environment;
                    const invocation = await invokeScheduledWorkflows(
                      event,
                      env,
                      scheduledWorkflows,
                    );
                    if (invocation.scheduledHandler)
                      throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
                  },
                  "scheduled",
                  wrapEnv,
                  cache,
                ),
              }
            : {}),
        };
  }
  return raw;
}

/** Validation checks the actual module namespace before returning its probe handler. */
export function validationHandler(tenant: Environment, name: string) {
  if (!(name in tenant)) throw new Error("missing entrypoint");
  return {
    fetch(): Response {
      return new Response("open-compute-validation-v1");
    },
  };
}

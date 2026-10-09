import entrypoints from "cloudflare-internal:workers";
import wrapped from "cloudflare-internal:wrapped-binding";
import {
  exports as currentExports,
  waitUntil,
  withEnv,
  withExports,
  WorkerEntrypoint,
} from "cloudflare:workers";
import type { CacheRuntime, CacheRuntimeFactory } from "../../cache/facade.js";
import { privateWeakMap } from "../../private-weak-map.js";
import {
  decodeServiceValue,
  encodeServiceValue,
  serviceCapabilityController,
} from "../../services/capabilities.js";
import { serviceRpcMember } from "../../services/rpc-member.js";
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
  Callable,
  CompletionReporter,
  Environment,
  TenantConstructor,
  TrackedContext,
} from "./types.js";

export { loopbackDurableObjectMetadata } from "./loopback.js";
export type {
  Environment,
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
const nativeApply = Reflect.apply;
const nativePush = Array.prototype.push;
const ownHas = Object.hasOwn;
const nativeGet = Reflect.get;
const NativeProxy = Proxy;
const nativeBind = Function.prototype.bind;
const nativeHas = Reflect.has;
const nativeOwnKeys = Reflect.ownKeys;
const nativeDescriptor = Reflect.getOwnPropertyDescriptor;
const nativeSet = Reflect.set;
const nativeReflectDefine = Reflect.defineProperty;
const nativeDelete = Reflect.deleteProperty;
const createServiceStub = wrapped.createServiceRpcStub.bind(wrapped);

const SERVICE_RPC = "__openComputeServiceRpc";
const SERVICE_GET = "__openComputeServiceGet";
const SCHEDULED_WORKFLOW_BINDING = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;
const RESERVED_METHODS = new Set([
  "constructor",
  "__proto__",
  "then",
  SERVICE_RPC,
  SERVICE_GET,
]);
interface NativeServiceContext {
  scopeId: string;
  frame: string;
  completion?: Fetcher;
}
const nativeServiceContexts = privateWeakMap<object, NativeServiceContext>();
const constructorFrames = privateWeakMap<TrackedContext, ServiceFrame>();

function serviceContext(ctx: object): {
  context: object;
  native?: NativeServiceContext;
} {
  const props: unknown = nativeGet(ctx, "props", ctx);
  if (props === null || typeof props !== "object") return { context: ctx };
  const native: unknown = nativeGet(props, "__OPEN_COMPUTE_SERVICE_CONTEXT");
  if (native === null || typeof native !== "object") return { context: ctx };
  const completion: unknown = nativeGet(native, "completion");
  if (
    completion !== undefined &&
    (completion === null ||
      typeof completion !== "object" ||
      !callable(nativeGet(completion, "fetch")))
  )
    throw new Error("SERVICE_BINDING_DENIED");
  if (
    typeof nativeGet(native, "scopeId") !== "string" ||
    typeof nativeGet(native, "frame") !== "string"
  )
    throw new Error("SERVICE_BINDING_DENIED");
  return {
    context: new NativeProxy(ctx, {
      get(target, property) {
        if (property === "props") return nativeGet(props, "userProps");
        const value: unknown = nativeGet(target, property, target);
        return callable(value)
          ? nativeApply(nativeBind, value, [target])
          : value;
      },
    }),
    native: {
      scopeId: nativeGet(native, "scopeId"),
      frame: nativeGet(native, "frame"),
      ...(completion === undefined
        ? {}
        : { completion: completion as Fetcher }),
    },
  };
}

async function nativeServiceFetch(
  owner: unknown,
  fn: Callable,
  request: Request,
  env: Environment,
  tracked: TrackedContext,
  native: NativeServiceContext,
  objectHandler: boolean,
  cache?: CacheRuntime,
): Promise<Response> {
  const completion = native.completion;
  if (!completion) throw new Error("SERVICE_BINDING_DENIED");
  constructorFrames.delete(tracked);
  tracked = takeDispatchContext(tracked);
  let drained: Promise<void> = Promise.resolve();
  let handoffWebSocket = false;
  try {
    const invokeOrigin = () =>
      withServiceScope(
        env,
        childServiceFrame(native.scopeId, native.frame),
        (scoped) =>
          withTenantEnvironment(
            scoped,
            () =>
              nativeApply(
                fn,
                owner,
                objectHandler ? [request, scoped, tracked.context] : [request],
              ),
            tracked,
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
    const background = drained.then(() => drainTrackedTasks(tracked));
    tracked.extendLifetime(
      handoffWebSocket
        ? background
        : background.then(async () => {
            const response = await completion.fetch(
              "https://service-completion.internal/",
            );
            if (!response.ok) throw new Error("SERVICE_UNAVAILABLE");
          }),
    );
  }
}

const trackedContexts = privateWeakMap<object, TrackedContext>();
const trackedInstances = privateWeakMap<object, TrackedContext>();
const instanceEnvironments = privateWeakMap<object, Environment>();
function callable(value: unknown): value is Callable {
  return typeof value === "function";
}

function preserveSubrequestLimit(error: unknown): Response {
  const message = String(error instanceof Error ? error.message : error);
  if (!/too many subrequests/i.test(message)) throw error;
  return new Response(null, {
    status: 500,
    headers: { "x-open-compute-resource-limit": "subrequests" },
  });
}

function catchSubrequestLimit(value: unknown): unknown {
  return value instanceof Promise
    ? value.catch(preserveSubrequestLimit)
    : value;
}

/** Read the full native export table before tenant export filtering begins. */
export function trustedContextExports(context: unknown): object | undefined {
  if (context === null || typeof context !== "object") return undefined;
  const value: unknown = nativeGet(context, "exports", context);
  return value !== null && typeof value === "object" ? value : undefined;
}

function withTenantEnvironment<T>(
  env: Environment,
  fn: () => T,
  tracked?: TrackedContext,
): T {
  const exports = tenantExports(currentExports);
  const run = () => withEnv(env, () => withExports(exports, fn)) as T;
  const tasks = tracked?.tasks;
  return tasks
    ? entrypoints.withWaitUntilObserver((promise) => {
        nativeApply(nativePush, tasks, [promise]);
      }, run)
    : run();
}

/** Validate only constructibility; tenant code still runs inside its isolate. */
export function tenantConstructor(value: unknown): TenantConstructor {
  if (!constructible(value)) throw new Error("missing entrypoint");
  // A scoped base keeps `super()` as a normal, type-checked constructor call.
  // Reflect.construct preserves new.target and the native inheritance chain.
  return new NativeProxy(value, {
    construct(target, args: unknown[], newTarget) {
      const env = args[1];
      if (env === null || typeof env !== "object" || Array.isArray(env))
        throw new Error("invalid tenant env");
      const ctx = args[0];
      const tracked =
        ctx !== null && typeof ctx === "object"
          ? trackedContexts.get(ctx)
          : undefined;
      const frame =
        (tracked && constructorFrames.get(tracked)) ?? rootServiceFrame();
      if (tracked) constructorFrames.set(tracked, frame);
      try {
        const instance: unknown = withServiceScope(
          env as Environment,
          frame,
          (scoped) =>
            withTenantEnvironment(
              scoped,
              () => Reflect.construct(target, args, newTarget),
              tracked,
            ),
        );
        if (
          instance === null ||
          (typeof instance !== "object" && typeof instance !== "function")
        ) {
          throw new Error("invalid tenant constructor result");
        }
        return instance;
      } catch (error) {
        if (frame.parentFrame === null)
          scheduleRootCompletion(env as Environment, frame, tracked);
        throw error;
      }
    },
  });
}

function constructible(value: unknown): value is TenantConstructor {
  if (typeof value !== "function") return false;
  const prototype: unknown = nativeGet(value, "prototype");
  if (prototype === null || typeof prototype !== "object") return false;
  try {
    Reflect.construct(Object, [], value);
    return true;
  } catch {
    return false;
  }
}

/** Track waitUntil work while preserving the native execution-context receiver. */
export function trackExecutionContext<Context extends object>(
  ctx: Context,
  cacheContext?: object,
  runScope?: <T>(fn: () => T) => T,
  trustedExports?: object,
): TrackedContext<Context> {
  const tasks: Promise<unknown>[] = [];
  const nativeWaitUntil: unknown = nativeGet(ctx, "waitUntil", ctx);
  const extendLifetime = callable(nativeWaitUntil)
    ? (promise: Promise<unknown>) => {
        nativeApply(nativeWaitUntil, ctx, [promise]);
      }
    : (promise: Promise<unknown>) => {
        waitUntil(promise);
      };
  const exports = tenantExports(trustedExports ?? currentExports);
  const trackedWaitUntil = (promise: Promise<unknown>) => {
    extendLifetime(Promise.resolve(promise));
  };
  const context = new NativeProxy(Object.create(null) as Context, {
    get(target, property, receiver) {
      if (property === "cache" && cacheContext !== undefined)
        return cacheContext;
      if (property === "exports") return exports;
      if (property === "waitUntil")
        return ownHas(target, property)
          ? nativeGet(target, property, receiver)
          : trackedWaitUntil;
      const value: unknown = nativeGet(ctx, property, ctx);
      return callable(value) ? nativeApply(nativeBind, value, [ctx]) : value;
    },
    has(target, property) {
      if (ownHas(target, property)) return true;
      if (property === "exports") return true;
      if (property === "cache" && cacheContext !== undefined) return true;
      return nativeHas(ctx, property);
    },
    ownKeys(target) {
      const keys = nativeOwnKeys(ctx);
      for (const key of nativeOwnKeys(target))
        if (!keys.includes(key)) keys.push(key);
      if (!keys.includes("exports")) keys.push("exports");
      return keys;
    },
    getOwnPropertyDescriptor(target, property) {
      if (ownHas(target, property)) return nativeDescriptor(target, property);
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
      const descriptor = nativeDescriptor(ctx, property);
      if (!descriptor) return undefined;
      const value: unknown = nativeGet(ctx, property, ctx);
      return {
        configurable: true,
        enumerable: descriptor.enumerable ?? false,
        writable: descriptor.writable ?? false,
        value:
          property === "waitUntil"
            ? trackedWaitUntil
            : callable(value)
              ? nativeApply(nativeBind, value, [ctx])
              : value,
      };
    },
    getPrototypeOf() {
      return null;
    },
    set(target, property, value) {
      if (property === "exports" || property === "cache") return false;
      if (property === "waitUntil")
        return nativeSet(target, property, value, target);
      return nativeSet(ctx, property, value, ctx);
    },
    defineProperty(target, property, descriptor) {
      if (property === "exports" || property === "cache") return false;
      if (property === "waitUntil")
        return nativeReflectDefine(target, property, descriptor);
      return nativeReflectDefine(ctx, property, descriptor);
    },
    deleteProperty(target, property) {
      if (property === "exports" || property === "cache") return false;
      if (property === "waitUntil") return nativeDelete(target, property);
      return nativeDelete(ctx, property);
    },
  });
  const tracked = {
    context,
    tasks,
    extendLifetime,
    ...(runScope === undefined ? {} : { runScope }),
  };
  trackedContexts.set(ctx, tracked);
  trackedContexts.set(context, tracked);
  return tracked;
}

function takeDispatchContext(tracked: TrackedContext): TrackedContext {
  const tasks = tracked.tasks;
  tracked.tasks = [];
  return { ...tracked, tasks };
}

function invoke(
  owner: unknown,
  fn: Callable,
  args: unknown[],
  env: Environment,
  trackedOverride?: TrackedContext,
): unknown {
  const source =
    trackedOverride ??
    (owner !== null && typeof owner === "object"
      ? trackedInstances.get(owner)
      : undefined);
  const frame = (source && constructorFrames.get(source)) ?? rootServiceFrame();
  if (source) constructorFrames.delete(source);
  const rootScope = frame.parentFrame === null ? frame : null;
  const tracked =
    source === undefined ? undefined : takeDispatchContext(source);
  const run = () => {
    try {
      const value = withServiceScope(env, frame, (scoped) =>
        withTenantEnvironment(
          scoped,
          () => nativeApply(fn, owner, args),
          tracked,
        ),
      );
      return rootResult(value, env, rootScope, tracked);
    } catch (error) {
      scheduleRootCompletion(env, rootScope, tracked);
      throw error;
    }
  };
  return tracked?.runScope ? tracked.runScope(run) : run();
}

function serviceMethod(owner: object, method: string): Callable {
  if (typeof method !== "string" || RESERVED_METHODS.has(method))
    throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
  return serviceRpcMember(owner, method, "call");
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
  constructorFrames.delete(tracked);
  tracked = takeDispatchContext(tracked);
  try {
    const controller = serviceCapabilityController(reporter, createServiceStub);
    const args = decodeServiceValue(
      rawArgs,
      privateWeakMap<object, unknown>(),
      controller,
    );
    if (!Array.isArray(args)) throw new Error("SERVICE_BINDING_DENIED");
    const value = await withServiceScope(env, frame, (scoped) =>
      withTenantEnvironment(
        scoped,
        async () => {
          const value = await nativeApply(
            serviceMethod(owner, method),
            owner,
            args,
          );
          return encodeServiceValue(value, controller);
        },
        tracked,
      ),
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
  constructorFrames.delete(tracked);
  tracked = takeDispatchContext(tracked);
  if (typeof property !== "string" || RESERVED_METHODS.has(property)) {
    throw new Error("SERVICE_ENTRYPOINT_NOT_FOUND");
  }
  try {
    const controller = serviceCapabilityController(reporter, createServiceStub);
    const value = await withServiceScope(env, frame, (scoped) =>
      withTenantEnvironment(
        scoped,
        async () => {
          const value = await serviceRpcMember(owner, property, "get");
          return encodeServiceValue(value, controller);
        },
        tracked,
      ),
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
  hostMethods?: Readonly<Record<string, unknown>>,
  beforeFetch?: (args: unknown[]) => (() => Promise<void>) | undefined,
): T {
  if (tracked) {
    trackedInstances.set(instance, tracked);
    instanceEnvironments.set(instance, env);
  }
  return new NativeProxy(instance, {
    get(target, property) {
      const value: unknown =
        hostMethods &&
        typeof property === "string" &&
        ownHas(hostMethods, property)
          ? hostMethods[property]
          : nativeGet(target, property, target);
      if (!callable(value)) return value;
      if (property === SERVICE_RPC || property === SERVICE_GET)
        return (...args: unknown[]) => nativeApply(value, target, args);
      return (...args: unknown[]) => {
        const native = nativeServiceContexts.get(target);
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
              () => nativeApply(value, target, args),
              args[0] as Request,
              tracked.context as ExecutionContext,
            );
          return invoke(target, operation, [], env, tracked);
        }
        const started = property === "fetch" ? beforeFetch?.(args) : undefined;
        let result: unknown;
        try {
          result = invoke(target, value, args, env, tracked);
        } finally {
          if (started) waitUntil(started().catch(() => undefined));
        }
        return property === "fetch" ? catchSubrequestLimit(result) : result;
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
    nativeGet(event, "type") !== undefined
  )
    return event;
  return new NativeProxy(event, {
    get(target, property) {
      if (property === "type") return "scheduled";
      const value: unknown = nativeGet(target, property, target);
      return callable(value) ? nativeApply(nativeBind, value, [target]) : value;
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
  const cron: unknown = nativeGet(event, "cron", event);
  const time: unknown = nativeGet(event, "scheduledTime", event);
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
  cache?: CacheRuntimeFactory,
) {
  return (event: unknown, env: Environment, ctx: ExecutionContext): unknown => {
    const boundCache = cache?.bind();
    const trustedExports = trustedContextExports(ctx);
    const wrapped = env;
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
          () => nativeApply(fn, owner, args),
          event,
          tracked.context as ExecutionContext,
        );
      return catchSubrequestLimit(
        invoke(owner, operation, [], wrapped, tracked),
      );
    }
    const result = invoke(owner, fn, args, wrapped, tracked);
    return kind === "fetch" ? catchSubrequestLimit(result) : result;
  };
}

/** Wrap class entrypoints without replacing their native inheritance chain. */
export function wrapEntrypoint(
  target: unknown,
  name?: string,
  cache?: CacheRuntimeFactory,
  scheduledWorkflows?: ScheduledWorkflowRuntime,
): TenantConstructor {
  const Base = tenantConstructor(target);
  const Wrapped = class extends Base {
    constructor(ctx: unknown, env: Environment) {
      if (ctx === null || typeof ctx !== "object")
        throw new Error("invalid execution context");
      const boundCache = cache?.bind();
      const trustedExports = trustedContextExports(ctx);
      const wrapped = env;
      const service = serviceContext(ctx);
      const tracked = trackExecutionContext(
        service.context as ExecutionContext,
        boundCache?.context,
        undefined,
        trustedExports,
      );
      if (service.native)
        constructorFrames.set(
          tracked,
          childServiceFrame(service.native.scopeId, service.native.frame),
        );
      super(tracked.context, wrapped);
      trackedInstances.set(this, tracked);
      instanceEnvironments.set(this, wrapped);
      if (service.native) nativeServiceContexts.set(this, service.native);
      const methods = { ...hostMethods };
      const scheduled = methods.scheduled;
      if (callable(scheduled)) {
        const observed: unknown = nativeGet(this, "scheduled", this);
        const original =
          observed === scheduled
            ? nativeGet(Base.prototype, "scheduled", this)
            : observed;
        methods.scheduled = (event: unknown) =>
          nativeApply(scheduled, this, [event, original]);
      } else {
        const original: unknown = nativeGet(this, "scheduled", this);
        if (callable(original))
          methods.scheduled = (event: unknown) =>
            nativeApply(original, this, [
              normalizedEvent("scheduled", event),
              wrapped,
              tracked.context,
            ]);
      }
      return wrapInstance(this, wrapped, tracked, boundCache, methods);
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
  // Capture host methods before a tenant constructor can return a Proxy that hides overrides.
  const hostMethods: Record<string, unknown> = {
    [SERVICE_RPC]: Wrapped.prototype[SERVICE_RPC],
    [SERVICE_GET]: Wrapped.prototype[SERVICE_GET],
  };
  if (name !== undefined)
    Object.defineProperty(Wrapped, "name", { value: name });
  if (scheduledWorkflows === undefined) return Wrapped;
  const Scheduled = class extends Wrapped {
    async scheduled(event: unknown, original: unknown): Promise<unknown> {
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
      if (!callable(original)) throw new Error("CRON_CUSTOM_EVENT_UNSUPPORTED");
      return invoke(
        this,
        original,
        [invocation.controller, environment, tracked.context],
        environment,
        tracked,
      );
    }
  };
  hostMethods.scheduled = Scheduled.prototype.scheduled;
  return Scheduled;
}

/** Give object/function-style defaults an env-aware private Service fetch entrypoint. */
export function wrapDefaultService(
  raw: unknown,
  cache?: CacheRuntimeFactory,
): TenantConstructor {
  if (
    callable(raw) &&
    /^\s*class\b/.test(Function.prototype.toString.call(raw))
  ) {
    return wrapEntrypoint(raw, "__OpenComputeDefaultService", cache);
  }
  const owner = raw !== null && typeof raw === "object" ? raw : undefined;
  const fetch = owner === undefined ? raw : nativeGet(owner, "fetch");
  const connectHandler =
    owner === undefined ? undefined : nativeGet(owner, "connect");
  return class OpenComputeDefaultService extends WorkerEntrypoint<Environment> {
    readonly #environment: Environment;
    readonly #tracked: TrackedContext;
    readonly #cache: CacheRuntime | undefined;

    constructor(ctx: unknown, env: Environment) {
      if (ctx === null || typeof ctx !== "object")
        throw new Error("invalid execution context");
      const boundCache = cache?.bind();
      const trustedExports = trustedContextExports(ctx);
      const wrapped = env;
      const service = serviceContext(ctx);
      const tracked = trackExecutionContext(
        service.context as ExecutionContext,
        boundCache?.context,
        undefined,
        trustedExports,
      );
      super(tracked.context, wrapped);
      if (service.native) nativeServiceContexts.set(this, service.native);
      this.#cache = boundCache;
      this.#environment = wrapped;
      this.#tracked = tracked;
    }

    async fetch(request: Request): Promise<Response> {
      const native = nativeServiceContexts.get(this);
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
      const handler: unknown = nativeGet(raw, key);
      if (callable(handler))
        result[key] = wrapHandler(raw, handler, key, cache);
    }
    if (scheduledWorkflows !== undefined) {
      const scheduled: unknown = nativeGet(raw, "scheduled");
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
          return nativeApply(scheduled, raw, [invocation.controller, env, ctx]);
        },
        "scheduled",
        cache,
      );
    }
    return result;
  }
  if (callable(raw)) {
    return /^\s*class\b/.test(Function.prototype.toString.call(raw))
      ? wrapEntrypoint(raw, undefined, cache, scheduledWorkflows)
      : {
          fetch: wrapHandler(undefined, raw, "fetch", cache),
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
                  cache,
                ),
              }
            : {}),
        };
  }
  return raw;
}

/** Validation checks the actual module namespace before returning its probe handler. */
export function validationHandler(
  tenant: Environment,
  name: string,
  requireConstructor: boolean,
) {
  if (
    !ownHas(tenant, name) ||
    (requireConstructor && !constructible(tenant[name]))
  )
    throw new Error("missing entrypoint");
  return {
    fetch(): Response {
      return new Response("open-compute-validation-v1");
    },
  };
}

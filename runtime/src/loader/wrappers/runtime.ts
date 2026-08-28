import { withEnv } from "cloudflare:workers";

/** Explicit tenant variables and product capabilities, never host service bindings. */
export type Environment = Record<string, unknown>;
export type EnvironmentWrapper = (env: Environment) => Environment;
export interface BindingFactory {
  names: readonly string[];
  create: new(raw: unknown, durableObject: boolean) => object;
}
interface TenantConstructor {
  new(ctx: unknown, env: Environment): object;
  readonly prototype: object;
}
type Callable = (this: unknown, ...args: unknown[]) => unknown;
const PRIVATE_ALARM_INDEX = "__OPEN_COMPUTE_PRIVATE_ALARM_INDEX";

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
      if (key !== PRIVATE_ALARM_INDEX) Object.defineProperty(out, key, {
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

function invoke(owner: unknown, fn: Callable, args: unknown[], env: Environment): unknown {
  return withEnv(env, () => Reflect.apply(fn, owner, args));
}

/** Preserve native/private-field receivers while restoring the tenant env scope. */
export function wrapInstance<T extends object>(instance: T, env: Environment): T {
  return new Proxy(instance, {
    get(target, property) {
      const value: unknown = Reflect.get(target, property, target);
      return callable(value) ? (...args: unknown[]) => invoke(target, value, args, env) : value;
    },
  });
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

function wrapHandler(owner: unknown, fn: Callable, kind: string, wrapEnv: EnvironmentWrapper) {
  return (event: unknown, env: Environment, ctx: ExecutionContext): unknown => {
    const wrapped = wrapEnv(env);
    return invoke(owner, fn, [normalizedEvent(kind, event), wrapped, ctx], wrapped);
  };
}

/** Wrap class entrypoints without replacing their native inheritance chain. */
export function wrapEntrypoint(target: unknown, wrapEnv: EnvironmentWrapper, name?: string): TenantConstructor {
  const Base = tenantConstructor(target);
  const Wrapped = class extends Base {
    constructor(ctx: unknown, env: Environment) {
      const wrapped = wrapEnv(env);
      super(ctx, wrapped);
      return wrapInstance(this, wrapped);
    }
  };
  if (name !== undefined) Object.defineProperty(Wrapped, "name", { value: name });
  return Wrapped;
}

/** Preserve object handlers, function-style fetch, and class-style Workers. */
export function wrapDefault(raw: unknown, wrapEnv: EnvironmentWrapper): unknown {
  if (raw !== null && typeof raw === "object") {
    const result: Environment = { ...raw };
    for (const key of ["fetch", "scheduled", "queue", "tail"]) {
      const handler: unknown = Reflect.get(raw, key);
      if (callable(handler)) result[key] = wrapHandler(raw, handler, key, wrapEnv);
    }
    return result;
  }
  if (callable(raw)) {
    return /^\s*class\b/.test(Function.prototype.toString.call(raw))
      ? wrapEntrypoint(raw, wrapEnv)
      : { fetch: wrapHandler(undefined, raw, "fetch", wrapEnv) };
  }
  return raw;
}

/** Validation checks the actual module namespace before returning its probe handler. */
export function validationHandler(tenant: Environment, name: string) {
  if (!(name in tenant)) throw new Error("missing entrypoint");
  return { fetch(): Response { return new Response("open-compute-validation-v1"); } };
}

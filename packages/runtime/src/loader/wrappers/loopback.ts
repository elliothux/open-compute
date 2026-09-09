import { WorkerEntrypoint } from "cloudflare:workers";
import type {
  Environment,
  EnvironmentWrapper,
  TenantConstructor,
} from "./runtime.js";

type Callable = (this: unknown, ...args: unknown[]) => unknown;
const PRIVATE_EXPORT_PREFIX = "__OpenCompute";
const BRIDGE = "__OpenComputeLoopbackService";
const loopbackNames = new Set<string>();
const ownedExports = new WeakSet<object>();

/** Reuse the normal environment/context wrapper for every native loopback service. */
export function createLoopbackEntrypoint(
  tenant: Environment,
  wrapEnv: EnvironmentWrapper,
  wrap: (
    target: unknown,
    environment: EnvironmentWrapper,
    name?: string,
  ) => TenantConstructor,
  alreadyWrapped: readonly string[] = [],
) {
  const constructors = new Map<string, TenantConstructor>();
  for (const [name, target] of Object.entries(tenant)) {
    if (
      !alreadyWrapped.includes(name) &&
      typeof target === "function" &&
      target.prototype instanceof WorkerEntrypoint
    ) {
      constructors.set(name, wrap(target, wrapEnv, name));
      loopbackNames.add(name);
    }
  }
  return class extends WorkerEntrypoint {
    constructor(ctx: ExecutionContext, env: Environment) {
      super(ctx, env);
      const options: unknown = ctx.props;
      if (options === null || typeof options !== "object")
        throw new Error("invalid loopback entrypoint");
      const name: unknown = Reflect.get(options, "name");
      const target =
        typeof name === "string" ? constructors.get(name) : undefined;
      if (!target) throw new Error("invalid loopback entrypoint");
      const props: unknown = Reflect.get(options, "props");
      const context = new Proxy(ctx, {
        get(owner, property) {
          if (property === "props") return props;
          const value: unknown = Reflect.get(owner, property, owner);
          return callable(value) ? value.bind(owner) : value;
        },
      });
      const instance = new target(context, env);
      if (!(instance instanceof WorkerEntrypoint))
        throw new Error("invalid loopback entrypoint");
      return instance;
    }
  };
}

function serviceExport(
  source: object,
  native: Callable,
  name: string,
): Callable {
  const bridge: unknown = Reflect.get(source, BRIDGE, source);
  if (!callable(bridge)) throw new Error("missing loopback entrypoint bridge");
  const scoped = (options: unknown): unknown => {
    if (
      options !== undefined &&
      options !== null &&
      typeof options !== "object" &&
      typeof options !== "function"
    ) {
      throw new TypeError("loopback options must be an object");
    }
    const original = options ?? {};
    const props: unknown = Reflect.get(original, "props");
    if (
      props !== undefined &&
      (props === null ||
        (typeof props !== "object" && typeof props !== "function"))
    ) {
      throw new TypeError("loopback props must be an object");
    }
    // Leave other native options (including date-gated version selection) to workerd.
    const scopedOptions = new Proxy(
      {},
      {
        get(_target, property) {
          return property === "props"
            ? { name, props: props === undefined ? {} : props }
            : Reflect.get(original, property, original);
        },
      },
    );
    return Reflect.apply(bridge, source, [scopedOptions]);
  };
  let unscoped: object | undefined;
  return new Proxy(native, {
    apply(_target, _receiver, args) {
      return scoped(args[0]);
    },
    get(_target, property) {
      if (!unscoped) {
        const value = scoped(undefined);
        if (
          value === null ||
          (typeof value !== "object" && typeof value !== "function")
        ) {
          throw new Error("invalid loopback entrypoint bridge");
        }
        unscoped = value;
      }
      const value: unknown = Reflect.get(unscoped, property, unscoped);
      return callable(value) ? value.bind(unscoped) : value;
    },
  });
}

const loopbackDurableObjects = new WeakMap<
  object,
  { entrypoint: string; props: unknown }
>();

function callable(value: unknown): value is Callable {
  return typeof value === "function";
}

/** Return the trusted export identity captured when a loopback class was constructed. */
export function loopbackDurableObjectMetadata(
  value: unknown,
): { entrypoint: string; props: unknown } | undefined {
  return value !== null &&
    (typeof value === "object" || typeof value === "function")
    ? loopbackDurableObjects.get(value)
    : undefined;
}

function privateExport(property: PropertyKey): boolean {
  return (
    typeof property === "string" && property.startsWith(PRIVATE_EXPORT_PREFIX)
  );
}

/** Expose public loopback entrypoints without leaking generated host bridges. */
export function tenantExports(source: object): object {
  const values = new Map<string, unknown>();
  const enumerable = new Map<string, boolean>();
  for (const property of Reflect.ownKeys(source)) {
    if (typeof property !== "string" || privateExport(property)) continue;
    const native: unknown = Reflect.get(source, property, source);
    values.set(
      property,
      loopbackNames.has(property) &&
        callable(native) &&
        !ownedExports.has(native)
        ? serviceExport(source, native, property)
        : native,
    );
    enumerable.set(
      property,
      Reflect.getOwnPropertyDescriptor(source, property)?.enumerable ?? false,
    );
  }
  const functions = new Map<string, Callable>();
  const exposed = (property: PropertyKey): unknown => {
    if (typeof property !== "string" || privateExport(property))
      return undefined;
    const value = values.get(property);
    if (!callable(value)) return value;
    const prior = functions.get(property);
    if (prior) return prior;
    const members = new Map<PropertyKey, Callable>();
    const bound = new Proxy(value, {
      apply(target, _receiver, args) {
        const result: unknown = Reflect.apply(target, source, args);
        if (
          !loopbackNames.has(property) &&
          result !== null &&
          (typeof result === "object" || typeof result === "function")
        ) {
          const options = args[0];
          const props =
            options !== null && typeof options === "object"
              ? Reflect.get(options, "props")
              : undefined;
          loopbackDurableObjects.set(
            result,
            Object.freeze({ entrypoint: property, props }),
          );
        }
        return result;
      },
      construct(target, args, newTarget) {
        return Reflect.construct(target, args, newTarget);
      },
      get(target, member) {
        const result: unknown = Reflect.get(target, member, target);
        if (!callable(result)) return result;
        const cached = members.get(member);
        if (cached) return cached;
        const method = new Proxy(result, {
          apply(operation, _receiver, args) {
            return Reflect.apply(operation, target, args);
          },
          construct(operation, args, newTarget) {
            return Reflect.construct(operation, args, newTarget);
          },
        });
        members.set(member, method);
        return method;
      },
    });
    ownedExports.add(bound);
    functions.set(property, bound);
    return bound;
  };
  return new Proxy(Object.create(null) as object, {
    get(_target, property) {
      return exposed(property);
    },
    has(_target, property) {
      return (
        typeof property === "string" &&
        !privateExport(property) &&
        values.has(property)
      );
    },
    ownKeys() {
      return [...values.keys()];
    },
    getOwnPropertyDescriptor(_target, property) {
      if (
        typeof property !== "string" ||
        privateExport(property) ||
        !values.has(property)
      )
        return undefined;
      return {
        configurable: true,
        enumerable: enumerable.get(property) ?? false,
        writable: false,
        value: exposed(property),
      };
    },
    getPrototypeOf() {
      return null;
    },
    set() {
      return false;
    },
    defineProperty() {
      return false;
    },
    deleteProperty() {
      return false;
    },
  });
}

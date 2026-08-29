import { env, withEnv } from "cloudflare:workers";

export interface ServiceFrame {
  readonly scopeId: string;
  readonly parentFrame: string | null;
}

const SCOPE = Symbol("open-compute-service-scope");
const scopes = new WeakMap<object, ServiceFrame>();

function object(value: unknown): value is object {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

/** Run tenant code with one unforgeable module-local Service frame association. */
export function withServiceScope<T>(
  userEnv: Record<string, unknown>,
  frame: ServiceFrame,
  action: (scopedEnv: Record<string, unknown>) => T,
): T {
  const scoped: Record<string, unknown> = {};
  for (const key of Reflect.ownKeys(userEnv)) {
    if (key === SCOPE) continue;
    const descriptor = Object.getOwnPropertyDescriptor(userEnv, key);
    if (descriptor) Object.defineProperty(scoped, key, descriptor);
  }
  const identity = Object.freeze({});
  scopes.set(identity, frame);
  Object.defineProperty(scoped, SCOPE, {
    value: identity,
    enumerable: false,
    configurable: false,
    writable: false,
  });
  return withEnv(scoped, () => action(scoped)) as T;
}

/** Return the current trusted frame; absence means code is outside a root event. */
export function currentServiceFrame(): ServiceFrame {
  if (!object(env)) throw new Error("SERVICE_BINDING_DENIED");
  const identity: unknown = Reflect.get(env, SCOPE);
  if (!object(identity) || !scopes.has(identity)) throw new Error("SERVICE_BINDING_DENIED");
  return scopes.get(identity)!;
}

/** Create one trusted root scope for a platform-dispatched event. */
export function rootServiceFrame(): ServiceFrame {
  return Object.freeze({ scopeId: crypto.randomUUID(), parentFrame: null });
}

/** Restore a child frame received only from the trusted Service controller. */
export function childServiceFrame(scopeId: string, parentFrame: string): ServiceFrame {
  if (!/^[0-9a-f-]{36}$/.test(scopeId) || !/^[0-9a-f-]{36}$/.test(parentFrame)) {
    throw new Error("SERVICE_BINDING_DENIED");
  }
  return Object.freeze({ scopeId, parentFrame });
}

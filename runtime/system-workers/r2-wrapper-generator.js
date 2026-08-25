export const R2_FACADE_MODULE = "__open_compute_r2_facade__.js";
export const R2_WRAPPER_MODULE = "__open_compute_r2_wrapper__.js";
export const R2_RESERVED_MODULES = Object.freeze([R2_FACADE_MODULE, R2_WRAPPER_MODULE]);

export function generateR2Wrapper(mainModule, bindingNames, entrypointName) {
  const main = JSON.stringify(`./${mainModule}`);
  const bindings = JSON.stringify(bindingNames);
  const named = entrypointName ? `
const NamedWrapped = ({
  [${JSON.stringify(entrypointName)}]: class extends tenant[${JSON.stringify(entrypointName)}] {
    constructor(ctx, env) {
      const wrapped = wrapEnv(env);
      withEnv(wrapped, () => super(ctx, wrapped));
      return wrapInstance(this, wrapped);
    }
  }
})[${JSON.stringify(entrypointName)}];
export { NamedWrapped as ${entrypointName} };
` : "";
  return `
import { withEnv } from "cloudflare:workers";
import { R2Bucket } from "./${R2_FACADE_MODULE}";
import * as tenant from ${main};
export * from ${main};
const R2_BINDINGS = ${bindings};
const wrappedMarker = Symbol("open-compute.r2-wrapped-env");
function wrapEnv(env) {
  if (!env || env[wrappedMarker]) return env;
  const out = { ...env };
  for (const name of R2_BINDINGS) out[name] = new R2Bucket(out[name]);
  Object.defineProperty(out, wrappedMarker, { value: true });
  return out;
}
function invoke(owner, fn, args, env) {
  return withEnv(env, () => Reflect.apply(fn, owner, args));
}
function wrapInstance(instance, env) {
  return new Proxy(instance, {
    get(target, property) {
      const value = Reflect.get(target, property, target);
      return typeof value === "function"
        ? (...args) => invoke(target, value, args, env)
        : value;
    }
  });
}
function wrapHandler(owner, fn) {
  return function(event, env, ctx) {
    const wrapped = wrapEnv(env);
    return invoke(owner, fn, [event, wrapped, ctx], wrapped);
  };
}
const raw = tenant.default;
let wrappedDefault = raw;
if (raw && typeof raw === "object") {
  wrappedDefault = { ...raw };
  for (const key of ["fetch", "scheduled", "queue", "tail"]) {
    if (typeof raw[key] === "function") wrappedDefault[key] = wrapHandler(raw, raw[key]);
  }
} else if (typeof raw === "function") {
  if (/^\\s*class\\b/.test(Function.prototype.toString.call(raw))) {
    wrappedDefault = class extends raw {
      constructor(ctx, env) {
        const wrapped = wrapEnv(env);
        withEnv(wrapped, () => super(ctx, wrapped));
        return wrapInstance(this, wrapped);
      }
    };
  } else {
    wrappedDefault = { fetch: wrapHandler(undefined, raw) };
  }
}
${named}
export default wrappedDefault;
`;
}

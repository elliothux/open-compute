export const R2_FACADE_MODULE = "__open_compute_r2_facade__.js";
export const D1_FACADE_MODULE = "__open_compute_d1_facade__.js";
export const DO_FACADE_MODULE = "__open_compute_do_facade__.js";
export const DO_ID_CODEC_MODULE = "__open_compute_do_id_codec__.js";
export const LOADED_ISOLATE_WRAPPER_MODULE = "__open_compute_loaded_isolate_wrapper__.js";
export const LOADED_ISOLATE_RESERVED_MODULES = Object.freeze([
  R2_FACADE_MODULE,
  D1_FACADE_MODULE,
  DO_FACADE_MODULE,
  DO_ID_CODEC_MODULE,
  LOADED_ISOLATE_WRAPPER_MODULE,
]);

export function generateBindingWrapper(
  mainModule,
  r2BindingNames,
  d1BindingNames,
  doBindingNames,
  entrypointName,
) {
  const main = JSON.stringify(`./${mainModule}`);
  const r2Bindings = JSON.stringify(r2BindingNames);
  const d1Bindings = JSON.stringify(d1BindingNames);
  const doBindings = JSON.stringify(doBindingNames);
  const imports = [
    r2BindingNames.length ? `import { R2Bucket } from "./${R2_FACADE_MODULE}";` : "",
    d1BindingNames.length ? `import { D1Database } from "./${D1_FACADE_MODULE}";` : "",
    doBindingNames.length ? `import { DurableObjectNamespace } from "./${DO_FACADE_MODULE}";` : "",
  ].join("\n");
  const wraps = [
    r2BindingNames.length ? "for (const name of R2_BINDINGS) out[name] = new R2Bucket(out[name]);" : "",
    d1BindingNames.length ? "for (const name of D1_BINDINGS) out[name] = new D1Database(out[name]);" : "",
    doBindingNames.length ? "for (const name of DO_BINDINGS) out[name] = new DurableObjectNamespace(out[name]);" : "",
  ].join("\n");
  const doContext = entrypointName ? `
function quoteSqlIdentifier(name) {
  return '"' + String(name).replaceAll('"', '""') + '"';
}
async function deleteAllDurableObjectStorage(storage) {
  const entries = await storage.list();
  const keys = [...entries.keys()];
  if (keys.length) await storage.delete(keys);
  const objects = [...storage.sql.exec(
    "SELECT type, name FROM sqlite_master " +
      "WHERE type IN ('trigger', 'view', 'table', 'index') " +
      "ORDER BY CASE type WHEN 'trigger' THEN 0 WHEN 'view' THEN 1 WHEN 'table' THEN 2 ELSE 3 END"
  )];
  storage.sql.exec("PRAGMA foreign_keys = OFF");
  try {
    for (const object of objects) {
      const type = String(object.type);
      const name = String(object.name);
      const lower = name.toLowerCase();
      if (lower.startsWith("sqlite_") || lower.startsWith("_cf_")) continue;
      if (!["trigger", "view", "table", "index"].includes(type)) continue;
      storage.sql.exec("DROP " + type.toUpperCase() + " IF EXISTS " + quoteSqlIdentifier(name));
    }
  } finally {
    storage.sql.exec("PRAGMA foreign_keys = ON");
  }
}
function wrapDurableObjectStorage(storage) {
  return new Proxy(storage, {
    get(target, property) {
      if (property === "deleteAll") {
        return async (...args) => {
          if (args.length > 1) throw new TypeError("deleteAll() accepts at most one options argument");
          await deleteAllDurableObjectStorage(target);
        };
      }
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? (...args) => Reflect.apply(value, target, args) : value;
    }
  });
}
function prepareDurableObjectContext(ctx) {
  if (!ctx?.storage) return ctx;
  const storage = wrapDurableObjectStorage(ctx.storage);
  try {
    Object.defineProperty(ctx, "storage", { value: storage, configurable: true });
    return ctx;
  } catch {
    return new Proxy(ctx, {
      get(target, property) {
        if (property === "storage") return storage;
        const value = Reflect.get(target, property, target);
        return typeof value === "function" ? (...args) => Reflect.apply(value, target, args) : value;
      }
    });
  }
}
` : "";
  const named = entrypointName ? `
const NamedWrapped = ({
  [${JSON.stringify(entrypointName)}]: class extends tenant[${JSON.stringify(entrypointName)}] {
    constructor(ctx, env) {
      const wrapped = wrapEnv(env);
      const wrappedCtx = prepareDurableObjectContext(ctx);
      withEnv(wrapped, () => super(wrappedCtx, wrapped));
      return wrapInstance(this, wrapped);
    }
  }
})[${JSON.stringify(entrypointName)}];
export { NamedWrapped as ${entrypointName} };
` : "";
  return `
import { withEnv } from "cloudflare:workers";
${imports}
import * as tenant from ${main};
export * from ${main};
${doContext}
const R2_BINDINGS = ${r2Bindings};
const D1_BINDINGS = ${d1Bindings};
const DO_BINDINGS = ${doBindings};
const wrappedMarker = Symbol("open-compute.loaded-isolate-wrapped-env");
function wrapEnv(env) {
  if (!env || env[wrappedMarker]) return env;
  const out = { ...env };
  ${wraps}
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
      return typeof value === "function" ? (...args) => invoke(target, value, args, env) : value;
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

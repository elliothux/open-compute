import type { RuntimeBinding, RuntimeServiceBinding } from "./protocol.js";
import type { generateBindingWrapper as WrapperGenerator } from "./wrappers/generator.js";

/** One root facade minted by the generated wrapper from a verified binding. */
export type ForwardingRoot =
  | {
      kind: "binding";
      descriptor: RuntimeBinding;
      transport: unknown;
      owner: object;
    }
  | {
      kind: "service";
      descriptor: RuntimeServiceBinding;
      transport: unknown;
      owner: object;
    }
  | {
      kind: "assets" | "images" | "ai";
      transport: unknown;
      owner: object;
    };

type PrivateCode = WorkerLoaderWorkerCode & {
  openComputePrivateEnv: Record<string, unknown>;
};

// This module is evaluated before tenant modules. Capture operations that tenant
// code may replace on global prototypes before the forwarding callback runs.
const NativeWeakMap = WeakMap;
const NativeMap = Map;
const apply = Reflect.apply;
const weakGet = WeakMap.prototype.get;
const weakSet = WeakMap.prototype.set;
const mapGet = Map.prototype.get;
const mapSet = Map.prototype.set;
const entries = Object.entries;
const descriptors = Object.getOwnPropertyDescriptors;
const define = Object.defineProperty;
const create = Object.create;
const isArray = Array.isArray;
const normalize = String.prototype.normalize;
const startsWith = String.prototype.startsWith;
const endsWith = String.prototype.endsWith;
const includes = String.prototype.includes;
const split = String.prototype.split;
const test = RegExp.prototype.test;

export function newWeakRegistry<V>(): WeakMap<object, V> {
  return new NativeWeakMap<object, V>();
}

export function registryGet<V>(
  registry: WeakMap<object, V>,
  key: object,
): V | undefined {
  return apply(weakGet, registry, [key]);
}

export function registrySet<V>(
  registry: WeakMap<object, V>,
  key: object,
  value: V,
): void {
  apply(weakSet, registry, [key, value]);
}

export function newDescriptorRegistry<V>(): Map<string, V> {
  return new NativeMap<string, V>();
}

export function descriptorGet<V>(
  registry: Map<string, V>,
  key: string,
): V | undefined {
  return apply(mapGet, registry, [key]);
}

export function descriptorSet<V>(
  registry: Map<string, V>,
  key: string,
  value: V,
): void {
  apply(mapSet, registry, [key, value]);
}

export function captureNativeLoader(loader: WorkerLoader): {
  get: WorkerLoader["get"];
  load: WorkerLoader["load"];
} {
  const prototype: unknown = Object.getPrototypeOf(loader);
  if (prototype === null || typeof prototype !== "object") denied();
  const get: unknown = Reflect.get(prototype, "get");
  const load: unknown = Reflect.get(prototype, "load");
  if (typeof get !== "function" || typeof load !== "function") denied();
  return {
    get: get as WorkerLoader["get"],
    load: load as WorkerLoader["load"],
  };
}

function denied(): never {
  throw new TypeError("WORKER_LOADER_FORWARDING_DENIED");
}

/** Build one wrapper-local forwarding path using the native Loader methods. */
export const createForwarding = (
  generateBindingWrapper: typeof WrapperGenerator,
  INTERNAL_MODULE_PREFIX: string,
  LOADED_ISOLATE_WRAPPER_MODULE: string,
  nativeLoader: { get: WorkerLoader["get"]; load: WorkerLoader["load"] },
) => {
  const BINDING_NAME = /^[A-Za-z_$][A-Za-z0-9_$]{0,63}$/;

  function validModuleName(name: string): boolean {
    const parts: string[] = apply(split, name, ["/"]);
    for (let index = 0; index < parts.length; index++) {
      if (parts[index] === "" || parts[index] === "." || parts[index] === "..")
        return false;
    }
    return (
      name.length > 0 &&
      name.length <= 512 &&
      name === apply(normalize, name, ["NFC"]) &&
      !apply(startsWith, name, ["/"]) &&
      !apply(endsWith, name, ["/"]) &&
      !apply(includes, name, ["\\"]) &&
      !apply(test, /[\x00-\x1f\x7f]/, [name]) &&
      !apply(startsWith, name, [INTERNAL_MODULE_PREFIX]) &&
      !apply(startsWith, name, ["open-compute:"])
    );
  }

  function forwardedCode(
    code: WorkerLoaderWorkerCode,
    roots: WeakMap<object, ForwardingRoot>,
    sources: Readonly<Record<string, string>>,
    owner: object,
  ): PrivateCode {
    const mainModule = code?.mainModule;
    const inputModules = code?.modules;
    const inputEnv: unknown = code?.env;
    if (
      code === null ||
      typeof code !== "object" ||
      typeof mainModule !== "string" ||
      !validModuleName(mainModule) ||
      inputModules === null ||
      typeof inputModules !== "object"
    )
      denied();
    if (
      inputEnv !== undefined &&
      (inputEnv === null || typeof inputEnv !== "object" || isArray(inputEnv))
    )
      denied();
    // workerd's dynamic-env serializer accepts ordinary records, but rejects
    // null-prototype objects as unsupported class instances.
    const publicEnv: Record<string, unknown> = {};
    const privateEnv: Record<string, unknown> = {};
    const bindings: RuntimeBinding[] = [];
    const services: RuntimeServiceBinding[] = [];
    let assetBindingName: string | undefined;
    let imagesBindingName: string | undefined;
    let aiBindingName: string | undefined;
    const properties = entries(descriptors(inputEnv ?? {}));
    for (let index = 0; index < properties.length; index++) {
      const entry = properties[index]!;
      const name = entry[0]!;
      const property = entry[1]!;
      if (
        apply(startsWith, name, ["__OPEN_COMPUTE_"]) ||
        apply(startsWith, name, ["__open_compute__"])
      )
        denied();
      if (!("value" in property)) denied();
      const value: unknown = property.value;
      const root =
        value !== null && typeof value === "object"
          ? registryGet(roots, value)
          : undefined;
      if (root === undefined) {
        define(publicEnv, name, { value, enumerable: true });
        continue;
      }
      if (root.owner !== owner) denied();
      if (
        !apply(test, BINDING_NAME, [name]) ||
        apply(startsWith, name, ["__"]) ||
        apply(startsWith, name, ["OPEN_COMPUTE_"])
      )
        denied();
      switch (root.kind) {
        case "binding":
          switch (root.descriptor.kind) {
            case "kv_namespace":
            case "r2_bucket":
            case "d1_database":
            case "do_namespace":
            case "queue_producer":
            case "workflow":
            case "vectorize_index":
            case "ai_search_namespace":
            case "ai_search_instance":
            case "artifacts_namespace":
              break;
            default:
              denied();
          }
          bindings[bindings.length] = { ...root.descriptor, name };
          break;
        case "service":
          services[services.length] = { ...root.descriptor, name };
          break;
        case "assets":
          if (assetBindingName !== undefined) denied();
          assetBindingName = name;
          break;
        case "images":
          if (imagesBindingName !== undefined) denied();
          imagesBindingName = name;
          break;
        case "ai":
          if (aiBindingName !== undefined) denied();
          aiBindingName = name;
          break;
        default:
          denied();
      }
      define(privateEnv, name, {
        value: root.transport,
        enumerable: true,
      });
    }
    const modules: WorkerLoaderWorkerCode["modules"] = create(null);
    const inputEntries = entries(inputModules);
    for (let index = 0; index < inputEntries.length; index++) {
      const entry = inputEntries[index]!;
      const name = entry[0]!;
      const value = entry[1];
      if (!validModuleName(name)) denied();
      define(modules, name, { value, enumerable: true });
    }
    const sourceEntries = entries(sources);
    for (let index = 0; index < sourceEntries.length; index++) {
      const entry = sourceEntries[index]!;
      const name = entry[0]!;
      const source = entry[1]!;
      define(modules, name, {
        value: { js: source },
        enumerable: true,
      });
    }
    define(modules, LOADED_ISOLATE_WRAPPER_MODULE, {
      value: {
        js: generateBindingWrapper({
          mainModule,
          bindings,
          services,
          durableObject: false,
          automaticCacheEnabled: false,
          cacheFailOpen: false,
          cacheTransportAvailable: false,
          forwardedChild: true,
          assetBindingName,
          imagesBindingName,
          aiBindingName,
        }),
      },
      enumerable: true,
    });
    return {
      ...code,
      mainModule: LOADED_ISOLATE_WRAPPER_MODULE,
      modules,
      env: publicEnv,
      openComputePrivateEnv: privateEnv,
    };
  }

  /** Preserve native get() laziness while fencing every source-version change.
   * Within one source version, callers must use a new ID whenever code, config,
   * or selected bindings change: a native cache hit never invokes callback.
   */
  function getWorker(
    loader: WorkerLoader,
    id: string,
    callback: () => WorkerLoaderWorkerCode | Promise<WorkerLoaderWorkerCode>,
    roots: WeakMap<object, ForwardingRoot>,
    sources: Readonly<Record<string, string>>,
    sourceIdentity: string,
    owner: object,
  ): WorkerStub {
    if (
      typeof id !== "string" ||
      id.length === 0 ||
      id.length > 512 ||
      typeof callback !== "function"
    )
      denied();
    return apply(nativeLoader.get, loader, [
      `${sourceIdentity}/${id}`,
      async () => forwardedCode(await callback(), roots, sources, owner),
    ]);
  }

  /** Load one dynamic Worker with only selected product roots in the private channel. */
  function loadWorker(
    loader: WorkerLoader,
    code: WorkerLoaderWorkerCode,
    roots: WeakMap<object, ForwardingRoot>,
    sources: Readonly<Record<string, string>>,
    owner: object,
  ): WorkerStub {
    return apply(nativeLoader.load, loader, [
      forwardedCode(code, roots, sources, owner),
    ]);
  }

  return { getWorker, loadWorker };
};

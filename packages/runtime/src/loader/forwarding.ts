import {
  nativeBinding,
  type NativeBinding,
} from "../bindings/native-construction.js";
import { PRIVATE_POLICY, type WorkerPolicy } from "./policy.js";
import type { RuntimeBinding, RuntimeServiceBinding } from "./protocol.js";

/** One root capability registered by the INTERNAL host policy from a verified binding. */
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
      kind: "assets" | "images" | "ai" | "browser";
      transport: unknown;
      owner: object;
    };

type PrivateCode = WorkerLoaderWorkerCode & {
  openComputePrivateEnv: Record<string, unknown>;
  openComputeBindings: Record<string, NativeBinding>;
  openComputeHostPolicy: true;
};

// This module is evaluated before tenant modules. Capture operations that tenant
// code may replace on global prototypes before the forwarding callback runs.
const NativeWeakMap = WeakMap;
const apply = Reflect.apply;
const weakGet = WeakMap.prototype.get;
const weakSet = WeakMap.prototype.set;
const entries = Object.entries;
const descriptors = Object.getOwnPropertyDescriptors;
const define = Object.defineProperty;
const setPrototype = Object.setPrototypeOf;
const create = Object.create;
const isArray = Array.isArray;
const normalize = String.prototype.normalize;
const startsWith = String.prototype.startsWith;
const endsWith = String.prototype.endsWith;
const includes = String.prototype.includes;
const split = String.prototype.split;
const test = RegExp.prototype.test;

function newWeakRegistry<V>(): WeakMap<object, V> {
  return new NativeWeakMap<object, V>();
}

function registryGet<V>(
  registry: WeakMap<object, V>,
  key: object,
): V | undefined {
  return apply(weakGet, registry, [key]);
}

function registrySet<V>(
  registry: WeakMap<object, V>,
  key: object,
  value: V,
): void {
  apply(weakSet, registry, [key, value]);
}

function captureNativeLoader(loader: WorkerLoader): {
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

/** Forward explicitly selected roots through a native private Loader grant. */
export const createForwarding = (nativeLoader: {
  get: WorkerLoader["get"];
  load: WorkerLoader["load"];
}) => {
  const INTERNAL_MODULE_PREFIX = "cloudflare-internal:";
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
    owner: object,
  ): PrivateCode {
    if (code === null || typeof code !== "object") denied();
    // Read tenant getters/proxies once. Never spread the mutable input again after granting host fields.
    const codeSnapshot = { ...code };
    const privateProperties = entries(descriptors(code));
    const snapshotProperties = entries(descriptors(codeSnapshot));
    for (let index = 0; index < privateProperties.length; index++) {
      if (apply(startsWith, privateProperties[index]![0], ["openCompute"]))
        denied();
    }
    for (let index = 0; index < snapshotProperties.length; index++) {
      if (apply(startsWith, snapshotProperties[index]![0], ["openCompute"]))
        denied();
    }
    const mainModule = codeSnapshot.mainModule;
    const inputModules = codeSnapshot.modules;
    const inputEnv: unknown = codeSnapshot.env;
    if (
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
    const nativeBindings: Record<string, NativeBinding> = {};
    const bindings: RuntimeBinding[] = [];
    const services: RuntimeServiceBinding[] = [];
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
          const native = nativeBinding(root.descriptor.kind, root.transport);
          if (native) {
            define(nativeBindings, name, { value: native, enumerable: true });
            if (native.kind !== "queue") continue;
          }
          break;
        case "service":
          services[services.length] = { ...root.descriptor, name };
          const service = nativeBinding("service", root.transport);
          if (!service) denied();
          define(nativeBindings, name, { value: service, enumerable: true });
          continue;
        case "assets":
        case "browser":
        case "images":
        case "ai":
          const nativeProduct = nativeBinding(root.kind, root.transport);
          if (!nativeProduct) denied();
          define(nativeBindings, name, {
            value: nativeProduct,
            enumerable: true,
          });
          continue;
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
    const policy: WorkerPolicy = {
      validation: false,
      durableObject: false,
      workflow: false,
      bindings,
      services,
      scheduledTargets: [],
      automaticCacheEnabled: false,
      cacheFailOpen: false,
      automaticCacheEntrypoints: [],
      workerLoaderNames: [],
      browserBindingNames: [],
    };
    define(privateEnv, PRIVATE_POLICY, { value: policy, enumerable: true });
    const result = {
      ...codeSnapshot,
      mainModule,
      modules,
      env: publicEnv,
      openComputePrivateEnv: privateEnv,
      openComputeBindings: nativeBindings,
      openComputeHostPolicy: true as const,
    };
    // JSG reads inherited struct fields too. Host-issued code has no tenant-controlled prototype.
    setPrototype(result, null);
    return result;
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
      async () => forwardedCode(await callback(), roots, owner),
    ]);
  }

  /** Load one dynamic Worker with only selected product roots in the private channel. */
  function loadWorker(
    loader: WorkerLoader,
    code: WorkerLoaderWorkerCode,
    roots: WeakMap<object, ForwardingRoot>,
    owner: object,
  ): WorkerStub {
    return apply(nativeLoader.load, loader, [
      forwardedCode(code, roots, owner),
    ]);
  }

  return { getWorker, loadWorker };
};

type RootDescriptor =
  | { kind: "binding"; descriptor: RuntimeBinding }
  | { kind: "service"; descriptor: RuntimeServiceBinding }
  | { kind: "assets" | "images" | "ai" | "browser" };
interface LoaderGrant {
  readonly loader: WorkerLoader;
  readonly owner: object;
  readonly roots: WeakMap<object, ForwardingRoot>;
  readonly sourceIdentity: string;
  readonly forward: ReturnType<typeof createForwarding>;
}
const loaderGrants = newWeakRegistry<LoaderGrant>();

/** Register capabilities before user initialization; no registry escapes the INTERNAL module. */
export function registerForwarding(
  publicEnv: Record<string, unknown>,
  privateEnv: Record<string, unknown>,
  policy: WorkerPolicy,
): void {
  if (policy.sourceIdentity === undefined) return;
  const owner = privateEnv.__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS;
  if (
    owner === null ||
    typeof owner !== "object" ||
    policy.workerLoaderNames.length === 0
  )
    denied();
  const roots = newWeakRegistry<ForwardingRoot>();
  const nativeBindings = privateEnv.__OPEN_COMPUTE_PRIVATE_NATIVE_BINDINGS;
  const root = (name: string, descriptor: RootDescriptor) => {
    const value = Reflect.get(publicEnv, name);
    if (value === null || typeof value !== "object") denied();
    const native =
      nativeBindings !== null && typeof nativeBindings === "object"
        ? Reflect.get(nativeBindings, name)
        : undefined;
    const transport: unknown =
      native !== null && typeof native === "object"
        ? Reflect.get(native, "fetcher")
        : Reflect.get(privateEnv, name);
    registrySet(roots, value, {
      ...descriptor,
      transport,
      owner,
    } as ForwardingRoot);
  };
  for (const binding of policy.bindings)
    root(binding.name, { kind: "binding", descriptor: binding });
  for (const service of policy.services)
    root(service.name, { kind: "service", descriptor: service });
  if (policy.assetBindingName !== undefined)
    root(policy.assetBindingName, { kind: "assets" });
  for (const name of policy.browserBindingNames)
    root(name, { kind: "browser" });
  if (policy.imagesBindingName !== undefined)
    root(policy.imagesBindingName, { kind: "images" });
  if (policy.aiBindingName !== undefined)
    root(policy.aiBindingName, { kind: "ai" });
  for (const name of policy.workerLoaderNames) {
    const loader: unknown = Reflect.get(publicEnv, name);
    const privateLoader: unknown = Reflect.get(owner, name);
    if (
      loader === null ||
      typeof loader !== "object" ||
      privateLoader === null ||
      typeof privateLoader !== "object"
    )
      denied();
    const nativeLoader = captureNativeLoader(privateLoader as WorkerLoader);
    registrySet(loaderGrants, loader, {
      loader: privateLoader as WorkerLoader,
      owner,
      roots,
      sourceIdentity: policy.sourceIdentity,
      forward: createForwarding(nativeLoader),
    });
  }
}
function grantFor(loader: WorkerLoader): LoaderGrant {
  const grant = registryGet(loaderGrants, loader);
  if (grant === undefined) denied();
  return grant;
}
/** Public helper accepts only the native Loader identity registered by the host policy. */
export function forwardGetWorker(
  loader: WorkerLoader,
  id: string,
  callback: () => WorkerLoaderWorkerCode | Promise<WorkerLoaderWorkerCode>,
): WorkerStub {
  const grant = grantFor(loader);
  return grant.forward.getWorker(
    grant.loader,
    id,
    callback,
    grant.roots,
    grant.sourceIdentity,
    grant.owner,
  );
}
/** Load a child without inheriting private policy, transports, or cache authority. */
export function forwardLoadWorker(
  loader: WorkerLoader,
  code: WorkerLoaderWorkerCode,
): WorkerStub {
  const grant = grantFor(loader);
  return grant.forward.loadWorker(grant.loader, code, grant.roots, grant.owner);
}

// Assemble only capabilities resolved and verified by RuntimeSource.
import {
  nativeBinding,
  type NativeBinding,
} from "../bindings/native-construction.js";
import type { DoPolicy } from "../durable-objects/protocol.js";
import { PRIVATE_POLICY, type WorkerPolicy } from "./policy.js";
import type {
  BindingContext,
  NativeWorkerLoaderFactory,
  RuntimeBinding,
  RuntimeModuleBinding,
  RuntimeSnapshot,
} from "./protocol.js";
import { bindingError } from "./shared.js";

function requireHostPolicy(loaderFactory: NativeWorkerLoaderFactory): void {
  if (loaderFactory.hostPolicyVersion !== 1)
    throw bindingError("RUNTIME_UNAVAILABLE");
}

function makeBinding(
  ctx: BindingContext,
  descriptor: RuntimeBinding,
  versionId: string,
  instanceId: string,
  workerId: string,
  policy: DoPolicy,
  durableObject: boolean,
): unknown {
  const identity = {
    bindingId: descriptor.bindingId,
    versionId,
    descriptorSha256: descriptor.descriptorSha256,
  };
  if (descriptor.capabilityVersion !== 1)
    throw bindingError("BINDING_CAPABILITY_UNSUPPORTED");
  if (descriptor.kind === "workflow") {
    const props = Object.freeze({ ...identity, durableObject });
    return ctx.exports.WorkflowBindingTransport({ props });
  }
  if (descriptor.kind === "queue_producer") {
    return ctx.exports.QueueTransport({
      props: Object.freeze({
        ...identity,
        instanceId,
        workerId,
        queueId: descriptor.queueId,
        queueLifecycleGeneration: descriptor.queueLifecycleGeneration,
        durableObject,
      }),
    });
  }
  const props = Object.freeze({
    ...identity,
    instanceId,
    workerId,
    namespaceResourceId: descriptor.resourceId,
    resourceSpecGeneration: descriptor.resourceSpecGeneration,
    permissions: Object.freeze({
      read: descriptor.permissions.read === true,
      write: descriptor.permissions.write === true,
    }),
  });
  switch (descriptor.kind) {
    case "kv_namespace":
      return ctx.exports.KVNamespace({ props });
    case "r2_bucket":
      return ctx.exports.R2Transport({ props });
    case "d1_database":
      return ctx.exports.D1Transport({ props });
    case "vectorize_index":
      return ctx.exports.VectorizeTransport({ props });
    case "ai_search_namespace":
    case "ai_search_instance":
      return ctx.exports.AiSearchTransport({ props });
    case "artifacts_namespace":
      return ctx.exports.ArtifactsTransport({ props });
    case "do_namespace": {
      if (
        typeof descriptor.namespacePrefix !== "string" ||
        !/^[0-9a-f]{16}$/.test(descriptor.namespacePrefix) ||
        typeof descriptor.namespaceNameKey !== "string"
      )
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      return Object.freeze({
        schemaVersion: 1,
        namespacePrefix: descriptor.namespacePrefix,
        namespaceNameKey: descriptor.namespaceNameKey,
        maxObjectNameBytes: policy.maxObjectNameBytes,
        transport: ctx.exports.DoTransport({ props }),
      });
    }
  }
}

function moduleBindingBytes(value: string): Uint8Array<ArrayBuffer> {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++)
    bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function makeModuleBinding(binding: RuntimeModuleBinding): unknown {
  const bytes = moduleBindingBytes(binding.bytesBase64);
  switch (binding.type) {
    case "text":
      return new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(
        bytes,
      );
    case "data":
      return bytes.buffer;
    case "wasm":
      return Reflect.construct(WebAssembly.Module, [
        bytes,
      ]) as WebAssembly.Module;
  }
}

/** Public Loader bindings needed to compile a Worker during admission. */
export function validationEnv(
  snapshot: RuntimeSnapshot,
  policy: WorkerPolicy,
  loaderFactory: NativeWorkerLoaderFactory,
) {
  requireHostPolicy(loaderFactory);
  const env: Record<string, unknown> = {};
  for (const binding of snapshot.workerLoaders) {
    Object.defineProperty(env, binding.name, {
      value: loaderFactory.get(binding.namespaceKey),
      enumerable: true,
    });
  }
  return {
    env,
    openComputeHostPolicy: true as const,
    openComputePrivateEnv: { [PRIVATE_POLICY]: policy },
  };
}

/** Keep raw product transports out of importable cloudflare:workers.env. */
export function tenantEnv(
  snapshot: RuntimeSnapshot,
  workerPolicy: WorkerPolicy,
  ctx: BindingContext,
  loaderFactory: NativeWorkerLoaderFactory,
  versionId: string,
  policy: DoPolicy,
  durableObject = false,
  currentEntrypoint = "default",
): {
  env: Record<string, unknown>;
  openComputePrivateEnv: Record<string, unknown>;
  openComputeBindings: Record<string, NativeBinding>;
  openComputeCache: Fetcher;
  openComputeHostPolicy: true;
} {
  requireHostPolicy(loaderFactory);
  const env = { ...snapshot.env };
  const openComputeBindings: Record<string, NativeBinding> = {};
  const privateNames = new Set<string>();
  const forwardingLoaders: Record<string, WorkerLoader> = {};
  const [instanceId, workerId] = snapshot.loaderKey.split("/");
  if (!instanceId || !workerId)
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  for (const binding of snapshot.workerLoaders) {
    if (Object.prototype.hasOwnProperty.call(env, binding.name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[binding.name] = loaderFactory.get(binding.namespaceKey);
    Object.defineProperty(forwardingLoaders, binding.name, {
      value: loaderFactory.getPrivate(binding.namespaceKey),
      enumerable: true,
    });
  }
  if (snapshot.workerLoaders.length > 0) {
    env.__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS = forwardingLoaders;
    privateNames.add("__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS");
  }
  for (const binding of snapshot.moduleBindings) {
    if (Object.prototype.hasOwnProperty.call(env, binding.name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[binding.name] = makeModuleBinding(binding);
  }
  for (const descriptor of snapshot.bindings) {
    if (Object.prototype.hasOwnProperty.call(env, descriptor.name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[descriptor.name] = makeBinding(
      ctx,
      descriptor,
      versionId,
      instanceId,
      workerId,
      policy,
      durableObject,
    );
    const native = nativeBinding(descriptor.kind, env[descriptor.name]);
    if (native) openComputeBindings[descriptor.name] = native;
    privateNames.add(descriptor.name);
  }
  if (snapshot.workerLoaders.length > 0) {
    env.__OPEN_COMPUTE_PRIVATE_NATIVE_BINDINGS = openComputeBindings;
    privateNames.add("__OPEN_COMPUTE_PRIVATE_NATIVE_BINDINGS");
  }
  if (snapshot.assetBinding) {
    const name = snapshot.assetBinding.name;
    if (Object.prototype.hasOwnProperty.call(env, name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.AssetTransport({
      props: Object.freeze({
        versionId,
        descriptorSha256: snapshot.workerCodeSha256,
      }),
    });
    const native = nativeBinding("assets", env[name]);
    if (!native) throw bindingError("VERSION_INVARIANT_VIOLATION");
    openComputeBindings[name] = native;
    privateNames.add(name);
  }
  for (const service of snapshot.services) {
    if (Object.prototype.hasOwnProperty.call(env, service.name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[service.name] = ctx.exports.ServiceTransport({
      props: Object.freeze({
        versionId,
        bindingName: service.name,
        descriptorSha256: service.descriptorSha256,
        ...(service.entrypoint === undefined
          ? {}
          : { entrypoint: service.entrypoint }),
      }),
    });
    const native = nativeBinding("service", env[service.name]);
    if (!native) throw bindingError("VERSION_INVARIANT_VIOLATION");
    openComputeBindings[service.name] = native;
    privateNames.add(service.name);
  }
  const cacheTransports: Record<string, Fetcher> = {};
  const defaultCachePolicy = {
    enabled: snapshot.cachePolicy.enabled,
    crossVersionCache: snapshot.cachePolicy.crossVersionCache,
  };
  for (const [cacheEntrypoint, selected] of Object.entries({
    default: defaultCachePolicy,
    ...snapshot.cachePolicy.entrypoints,
    [currentEntrypoint]:
      snapshot.cachePolicy.entrypoints[currentEntrypoint] ?? defaultCachePolicy,
  })) {
    cacheTransports[cacheEntrypoint] = ctx.exports.CacheTransport({
      props: Object.freeze({
        instanceId,
        workerId,
        versionId,
        entrypoint: cacheEntrypoint,
        descriptorSha256: snapshot.workerCodeSha256,
        automaticEnabled: selected.enabled,
        crossVersionCache: selected.crossVersionCache,
      }),
    });
  }
  Object.defineProperty(env, "__OPEN_COMPUTE_PRIVATE_CACHE", {
    value: Object.freeze(cacheTransports),
    enumerable: true,
    configurable: true,
    writable: false,
  });
  privateNames.add("__OPEN_COMPUTE_PRIVATE_CACHE");
  for (const { name, descriptorSha256 } of snapshot.browserBindings) {
    if (Object.prototype.hasOwnProperty.call(env, name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.BrowserTransport({
      props: Object.freeze({
        instanceId,
        workerId,
        versionId,
        deploymentId: snapshot.observability?.deploymentId,
        bindingName: name,
        descriptorSha256,
        capabilityVersion: 1,
      }),
    });
    const native = nativeBinding("browser", env[name]);
    if (!native) throw bindingError("VERSION_INVARIANT_VIOLATION");
    openComputeBindings[name] = native;
    privateNames.add(name);
  }
  if (snapshot.imagesBinding) {
    const { name, descriptorSha256 } = snapshot.imagesBinding;
    if (Object.prototype.hasOwnProperty.call(env, name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.ImageTransport({
      props: Object.freeze({
        instanceId,
        workerId,
        versionId,
        descriptorSha256,
      }),
    });
    const native = nativeBinding("images", env[name]);
    if (!native) throw bindingError("VERSION_INVARIANT_VIOLATION");
    openComputeBindings[name] = native;
    privateNames.add(name);
  }
  if (snapshot.aiBinding) {
    const { name, descriptorSha256 } = snapshot.aiBinding;
    if (Object.prototype.hasOwnProperty.call(env, name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.AiTransport({
      props: Object.freeze({
        instanceId,
        workerId,
        versionId,
        descriptorSha256,
      }),
    });
    const native = nativeBinding("ai", env[name]);
    if (!native) throw bindingError("VERSION_INVARIANT_VIOLATION");
    openComputeBindings[name] = native;
    privateNames.add(name);
  }
  if (snapshot.versionMetadataBinding) {
    const metadata = snapshot.versionMetadataBinding;
    if (Object.prototype.hasOwnProperty.call(env, metadata.name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    const timestamp = new Date(metadata.timestampMs).toISOString();
    env[metadata.name] = Object.freeze({
      id: metadata.id,
      tag: metadata.tag ?? "",
      timestamp,
    });
  }
  const publicEnv: Record<string, unknown> = {};
  const privateEnv: Record<string, unknown> = {
    [PRIVATE_POLICY]: workerPolicy,
  };
  for (const [name, value] of Object.entries(env)) {
    const native = openComputeBindings[name];
    // Queue's native serializer delegates durable publication to its private caller policy.
    if (native && native.kind !== "queue") continue;
    Object.defineProperty(
      privateNames.has(name) ? privateEnv : publicEnv,
      name,
      {
        value,
        enumerable: true,
      },
    );
  }
  return {
    env: publicEnv,
    openComputePrivateEnv: privateEnv,
    openComputeBindings,
    openComputeCache: cacheTransports[currentEntrypoint]!,
    openComputeHostPolicy: true,
  };
}

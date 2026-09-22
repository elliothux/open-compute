// Assemble only capabilities resolved and verified by RuntimeSource.
import type { DoPolicy } from "../durable-objects/protocol.js";
import type {
  BindingContext,
  NativeWorkerLoaderFactory,
  RuntimeBinding,
  RuntimeModuleBinding,
  RuntimeSnapshot,
} from "./protocol.js";
import { bindingError } from "./shared.js";

function makeBinding(
  ctx: BindingContext,
  descriptor: RuntimeBinding,
  versionId: string,
  accountId: string,
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
        accountId,
        workerId,
        queueId: descriptor.queueId,
        queueLifecycleGeneration: descriptor.queueLifecycleGeneration,
      }),
    });
  }
  const props = Object.freeze({
    ...identity,
    accountId,
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
  loaderFactory: NativeWorkerLoaderFactory,
): Record<string, unknown> {
  const env: Record<string, unknown> = {};
  for (const binding of snapshot.workerLoaders) {
    Object.defineProperty(env, binding.name, {
      value: loaderFactory.get(binding.namespaceKey),
      enumerable: true,
    });
  }
  return env;
}

/** Keep raw product transports out of importable cloudflare:workers.env. */
export function tenantEnv(
  snapshot: RuntimeSnapshot,
  ctx: BindingContext,
  loaderFactory: NativeWorkerLoaderFactory,
  versionId: string,
  policy: DoPolicy,
  durableObject = false,
  currentEntrypoint = "default",
): {
  env: Record<string, unknown>;
  openComputePrivateEnv: Record<string, unknown>;
} {
  const env = { ...snapshot.env };
  const privateNames = new Set<string>();
  const forwardingLoaders: Record<string, WorkerLoader> = {};
  const [accountId, workerId] = snapshot.loaderKey.split("/");
  if (!accountId || !workerId)
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
      accountId,
      workerId,
      policy,
      durableObject,
    );
    privateNames.add(descriptor.name);
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
    privateNames.add(service.name);
  }
  const cacheTransports: Record<string, unknown> = {};
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
        accountId,
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
  if (snapshot.imagesBinding) {
    const { name, descriptorSha256 } = snapshot.imagesBinding;
    if (Object.prototype.hasOwnProperty.call(env, name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.ImageTransport({
      props: Object.freeze({
        accountId,
        workerId,
        versionId,
        descriptorSha256,
      }),
    });
    privateNames.add(name);
  }
  if (snapshot.aiBinding) {
    const { name, descriptorSha256 } = snapshot.aiBinding;
    if (Object.prototype.hasOwnProperty.call(env, name))
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.AiTransport({
      props: Object.freeze({
        accountId,
        workerId,
        versionId,
        descriptorSha256,
      }),
    });
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
  const privateEnv: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(env)) {
    Object.defineProperty(
      privateNames.has(name) ? privateEnv : publicEnv,
      name,
      {
        value,
        enumerable: true,
      },
    );
  }
  return { env: publicEnv, openComputePrivateEnv: privateEnv };
}

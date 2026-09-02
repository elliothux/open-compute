// Assemble only capabilities resolved and verified by RuntimeSource.
import { bindingError } from "./host.js";
import type { BindingContext, RuntimeBinding, RuntimeSnapshot } from "./protocol.js";
import type { DoPolicy } from "../durable-objects/protocol.js";

function makeBinding(ctx: BindingContext, descriptor: RuntimeBinding, versionId: string,
  routeGeneration: number, accountId: string, workerId: string, policy: DoPolicy, durableObject: boolean): unknown {
  const identity = { bindingId: descriptor.bindingId, versionId, descriptorSha256: descriptor.descriptorSha256 };
  if (descriptor.capabilityVersion !== 1) throw bindingError("BINDING_CAPABILITY_UNSUPPORTED");
  if (descriptor.kind === "workflow") {
    const props = Object.freeze({ ...identity, durableObject });
    return ctx.exports.WorkflowBindingTransport({ props });
  }
  if (descriptor.kind === "queue_producer") {
    return ctx.exports.QueueTransport({ props: Object.freeze({
      ...identity, accountId, workerId, queueId: descriptor.queueId,
      queueLifecycleGeneration: descriptor.queueLifecycleGeneration,
    }) });
  }
  const props = Object.freeze({
    ...identity, accountId, workerId, routeGeneration,
    namespaceResourceId: descriptor.resourceId,
    resourceSpecGeneration: descriptor.resourceSpecGeneration,
    permissions: Object.freeze({ read: descriptor.permissions.read === true, write: descriptor.permissions.write === true }),
  });
  switch (descriptor.kind) {
    case "kv_namespace": return ctx.exports.KVNamespace({ props });
    case "r2_bucket": return ctx.exports.R2Transport({ props });
    case "d1_database": return ctx.exports.D1Transport({ props });
    case "vectorize_index": return ctx.exports.VectorizeTransport({ props });
    case "ai_search_namespace": case "ai_search_instance":
      return ctx.exports.AiSearchTransport({ props });
    case "do_namespace": {
      if (typeof descriptor.namespacePrefix !== "string" || !/^[0-9a-f]{16}$/.test(descriptor.namespacePrefix)
          || typeof descriptor.namespaceNameKey !== "string") throw bindingError("VERSION_INVARIANT_VIOLATION");
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

export function tenantEnv(snapshot: RuntimeSnapshot, ctx: BindingContext, versionId: string,
  policy: DoPolicy, durableObject = false, builtinFeatures = true,
  currentEntrypoint = "default"): Record<string, unknown> {
  const env = { ...snapshot.env };
  const [accountId, workerId] = snapshot.loaderKey.split("/");
  if (!accountId || !workerId) throw bindingError("VERSION_INVARIANT_VIOLATION");
  for (const descriptor of snapshot.bindings) {
    if (Object.prototype.hasOwnProperty.call(env, descriptor.name)) throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[descriptor.name] = makeBinding(ctx, descriptor, versionId, snapshot.routeGeneration,
      accountId, workerId, policy, durableObject);
  }
  if (snapshot.assetBinding) {
    const name = snapshot.assetBinding.name;
    if (Object.prototype.hasOwnProperty.call(env, name)) throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[name] = ctx.exports.AssetTransport({ props: Object.freeze({
      versionId,
      descriptorSha256: snapshot.workerCodeSha256,
    }) });
  }
  for (const service of snapshot.services) {
    if (Object.prototype.hasOwnProperty.call(env, service.name)) throw bindingError("VERSION_INVARIANT_VIOLATION");
    env[service.name] = ctx.exports.ServiceTransport({ props: Object.freeze({
      versionId,
      bindingName: service.name,
      descriptorSha256: service.descriptorSha256,
      ...(service.entrypoint === undefined ? {} : { entrypoint: service.entrypoint }),
    }) });
  }
  if (builtinFeatures && !durableObject) {
    const cacheTransports: Record<string, unknown> = {};
    const defaultCachePolicy = {
      enabled: snapshot.cachePolicy.enabled,
      crossVersionCache: snapshot.cachePolicy.crossVersionCache,
    };
    for (const [cacheEntrypoint, selected] of Object.entries({
      default: defaultCachePolicy,
      ...snapshot.cachePolicy.entrypoints,
      [currentEntrypoint]: snapshot.cachePolicy.entrypoints[currentEntrypoint] ?? defaultCachePolicy,
    })) {
      cacheTransports[cacheEntrypoint] = ctx.exports.CacheTransport({ props: Object.freeze({
        accountId, workerId, versionId, entrypoint: cacheEntrypoint,
        descriptorSha256: snapshot.workerCodeSha256,
        automaticEnabled: selected.enabled,
        crossVersionCache: selected.crossVersionCache,
      }) });
    }
    Object.defineProperty(env, "__OPEN_COMPUTE_PRIVATE_CACHE", {
      value: Object.freeze(cacheTransports),
      enumerable: true,
      configurable: true,
      writable: false,
    });
    if (snapshot.imagesBinding) {
      const { name, descriptorSha256 } = snapshot.imagesBinding;
      if (Object.prototype.hasOwnProperty.call(env, name)) throw bindingError("VERSION_INVARIANT_VIOLATION");
      env[name] = ctx.exports.ImageTransport({ props: Object.freeze({
        accountId, workerId, versionId, descriptorSha256,
      }) });
    }
    if (snapshot.aiBinding) {
      const { name, descriptorSha256 } = snapshot.aiBinding;
      if (Object.prototype.hasOwnProperty.call(env, name)) throw bindingError("VERSION_INVARIANT_VIOLATION");
      env[name] = ctx.exports.AiTransport({ props: Object.freeze({
        accountId, workerId, versionId, descriptorSha256,
      }) });
    }
    if (snapshot.versionMetadataBinding) {
      const metadata = snapshot.versionMetadataBinding;
      if (Object.prototype.hasOwnProperty.call(env, metadata.name)) throw bindingError("VERSION_INVARIANT_VIOLATION");
      const timestamp = new Date(metadata.timestampMs).toISOString();
      env[metadata.name] = Object.freeze({
        id: metadata.id,
        tag: metadata.tag ?? "",
        timestamp,
      });
    }
  }
  return env;
}

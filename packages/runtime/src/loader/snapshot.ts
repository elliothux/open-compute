import type { RuntimeSnapshot } from "./protocol.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function invalid(): never {
  throw Object.assign(new Error("VERSION_INVARIANT_VIOLATION"), { stableCode: "VERSION_INVARIANT_VIOLATION" });
}

function jsonValue(value: unknown, depth: number): boolean {
  if (depth > 32) return false;
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(item => jsonValue(item, depth + 1));
  if (!record(value)
      || (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null)) return false;
  return Object.values(value).every(item => jsonValue(item, depth + 1));
}

/** Check the internal wire shape; Rust remains the authority for identity and policy. */
export function assertSnapshot(value: unknown): asserts value is RuntimeSnapshot {
  if (!record(value) || value.schemaVersion !== 1 || typeof value.loaderKey !== "string"
      || typeof value.workerCodeSha256 !== "string" || typeof value.routeGeneration !== "number"
      || !Number.isSafeInteger(value.routeGeneration) || value.routeGeneration < 0
      || !["worker", "assets_only"].includes(String(value.contentKind))
      || (value.contentKind === "worker" && typeof value.mainModule !== "string")
      || (value.contentKind === "assets_only" && value.mainModule !== undefined)
      || !Array.isArray(value.modules)
      || !record(value.env) || !Array.isArray(value.bindings) || !Array.isArray(value.services)
      || !record(value.cachePolicy) || typeof value.cachePolicy.enabled !== "boolean"
      || typeof value.cachePolicy.failOpen !== "boolean"
      || typeof value.cachePolicy.crossVersionCache !== "boolean"
      || !record(value.cachePolicy.entrypoints)) invalid();
  for (const policy of Object.values(value.cachePolicy.entrypoints as Record<string, unknown>)) {
    if (!record(policy) || typeof policy.enabled !== "boolean"
        || typeof policy.crossVersionCache !== "boolean") invalid();
  }
  if (value.observability !== undefined) {
    const item = value.observability;
    if (!record(item) || item.schemaVersion !== 1
        || typeof item.accountId !== "string" || typeof item.workerId !== "string"
        || typeof item.scriptName !== "string" || typeof item.versionId !== "string"
        || (item.deploymentId !== undefined && typeof item.deploymentId !== "string")
        || typeof item.routeGeneration !== "number"
        || !Number.isSafeInteger(item.routeGeneration) || item.routeGeneration < 1
        || typeof item.observabilityGeneration !== "number"
        || !Number.isSafeInteger(item.observabilityGeneration) || item.observabilityGeneration < 1
        || typeof item.enabled !== "boolean" || typeof item.logsEnabled !== "boolean"
        || typeof item.headSamplingRate !== "number" || !Number.isFinite(item.headSamplingRate)
        || item.headSamplingRate < 0 || item.headSamplingRate > 1
        || typeof item.invocationLogs !== "boolean" || typeof item.persist !== "boolean"
        || item.routeGeneration !== value.routeGeneration) {
      invalid();
    }
  }
  if (value.imagesBinding !== undefined
      && (!record(value.imagesBinding) || typeof value.imagesBinding.name !== "string"
        || typeof value.imagesBinding.descriptorSha256 !== "string")) invalid();
  if (value.aiBinding !== undefined
      && (!record(value.aiBinding) || typeof value.aiBinding.name !== "string"
        || typeof value.aiBinding.descriptorSha256 !== "string")) invalid();
  if (value.versionMetadataBinding !== undefined
      && (!record(value.versionMetadataBinding) || typeof value.versionMetadataBinding.name !== "string"
        || typeof value.versionMetadataBinding.id !== "string"
        || typeof value.versionMetadataBinding.timestampMs !== "number"
        || !Number.isSafeInteger(value.versionMetadataBinding.timestampMs)
        || typeof value.versionMetadataBinding.descriptorSha256 !== "string"
        || (value.versionMetadataBinding.tag !== undefined
          && typeof value.versionMetadataBinding.tag !== "string"))) invalid();
  if (value.assetBinding !== undefined
      && (!record(value.assetBinding) || typeof value.assetBinding.name !== "string")) invalid();
  if (value.assets !== undefined) {
    if (!record(value.assets) || !record(value.assets.manifest) || !record(value.assets.routing)
        || value.assets.manifest.schemaVersion !== 1 || !Array.isArray(value.assets.manifest.entries)
        || value.assets.routing.schemaVersion !== 1) invalid();
    for (const entry of value.assets.manifest.entries as unknown[]) {
      if (!record(entry) || typeof entry.path !== "string" || typeof entry.sha256 !== "string"
          || typeof entry.size !== "number" || !Number.isSafeInteger(entry.size)
          || typeof entry.contentType !== "string") invalid();
    }
  }
  if (value.contentKind === "assets_only" && value.assets === undefined) invalid();
  for (const module of value.modules as unknown[]) {
    if (!record(module) || typeof module.name !== "string" || typeof module.bytesBase64 !== "string"
        || typeof module.type !== "string"
        || !["esModule", "commonJsModule", "text", "json", "data", "wasm"].includes(module.type)) invalid();
  }
  for (const binding of value.bindings as unknown[]) {
    if (!record(binding) || typeof binding.name !== "string" || typeof binding.bindingId !== "string"
        || typeof binding.descriptorSha256 !== "string" || typeof binding.capabilityVersion !== "number"
        || !Number.isSafeInteger(binding.capabilityVersion)) invalid();
    switch (binding.kind) {
      case "workflow": break;
      case "queue_producer":
        if (typeof binding.queueId !== "string" || typeof binding.queueLifecycleGeneration !== "number"
            || !Number.isSafeInteger(binding.queueLifecycleGeneration)) invalid();
        break;
      case "kv_namespace": case "r2_bucket": case "d1_database": case "do_namespace":
      case "vectorize_index": case "ai_search_namespace": case "ai_search_instance":
        if (typeof binding.resourceId !== "string" || typeof binding.resourceSpecGeneration !== "number"
            || !Number.isSafeInteger(binding.resourceSpecGeneration) || !record(binding.permissions)
            || typeof binding.permissions.read !== "boolean" || typeof binding.permissions.write !== "boolean"
            || (binding.namespacePrefix !== undefined && typeof binding.namespacePrefix !== "string")
            || (binding.namespaceNameKey !== undefined && typeof binding.namespaceNameKey !== "string")) invalid();
        break;
      default: invalid();
    }
  }
  for (const service of value.services as unknown[]) {
    if (!record(service) || service.schemaVersion !== 1 || service.policyVersion !== 1
        || typeof service.name !== "string" || typeof service.targetWorkerId !== "string"
        || typeof service.descriptorSha256 !== "string"
        || (service.entrypoint !== undefined && typeof service.entrypoint !== "string")
        || (service.props !== undefined && (!record(service.props) || !jsonValue(service.props, 0)
          || new TextEncoder().encode(JSON.stringify(service.props)).byteLength > 64 * 1024))) invalid();
  }
}

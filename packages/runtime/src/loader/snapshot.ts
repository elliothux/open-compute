import type { RuntimeSnapshot } from "./protocol.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function strings(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item: unknown) => typeof item === "string");
}

function invalid(): never {
  throw Object.assign(new Error("DEPLOYMENT_INVARIANT_VIOLATION"), { stableCode: "DEPLOYMENT_INVARIANT_VIOLATION" });
}

/** Check the internal wire shape; Rust remains the authority for identity and policy. */
export function assertSnapshot(value: unknown): asserts value is RuntimeSnapshot {
  if (!record(value) || value.schemaVersion !== 1 || typeof value.loaderKey !== "string"
      || typeof value.workerCodeSha256 !== "string" || typeof value.routeGeneration !== "number"
      || !Number.isSafeInteger(value.routeGeneration) || value.routeGeneration < 0
      || !["worker", "assets_only"].includes(String(value.contentKind))
      || (value.contentKind === "worker" && typeof value.mainModule !== "string")
      || (value.contentKind === "assets_only" && value.mainModule !== undefined)
      || typeof value.compatibilityDate !== "string"
      || !strings(value.compatibilityFlags) || !Array.isArray(value.modules)
      || !record(value.env) || !Array.isArray(value.bindings) || !Array.isArray(value.services)) invalid();
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
        || (service.entrypoint !== undefined && typeof service.entrypoint !== "string")) invalid();
  }
}

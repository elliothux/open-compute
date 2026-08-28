// Generated from runtime/src/loader/snapshot.ts by Rolldown. Do not edit.
function record(value) {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
function strings(value) {
	return Array.isArray(value) && value.every((item) => typeof item === "string");
}
function invalid() {
	throw Object.assign(new Error("DEPLOYMENT_INVARIANT_VIOLATION"), { stableCode: "DEPLOYMENT_INVARIANT_VIOLATION" });
}
/** Check the internal wire shape; Rust remains the authority for identity and policy. */
export function assertSnapshot(value) {
	if (!record(value) || value.schemaVersion !== 1 || typeof value.loaderKey !== "string" || typeof value.workerCodeSha256 !== "string" || typeof value.routeGeneration !== "number" || !Number.isSafeInteger(value.routeGeneration) || value.routeGeneration < 0 || typeof value.mainModule !== "string" || typeof value.compatibilityDate !== "string" || !strings(value.compatibilityFlags) || !Array.isArray(value.modules) || !record(value.env) || !Array.isArray(value.bindings)) invalid();
	for (const module of value.modules) {
		if (!record(module) || typeof module.name !== "string" || typeof module.bytesBase64 !== "string" || typeof module.type !== "string" || ![
			"esModule",
			"commonJsModule",
			"text",
			"json",
			"data",
			"wasm"
		].includes(module.type)) invalid();
	}
	for (const binding of value.bindings) {
		if (!record(binding) || typeof binding.name !== "string" || typeof binding.bindingId !== "string" || typeof binding.descriptorSha256 !== "string" || typeof binding.capabilityVersion !== "number" || !Number.isSafeInteger(binding.capabilityVersion)) invalid();
		switch (binding.kind) {
			case "workflow": break;
			case "queue_producer":
				if (typeof binding.queueId !== "string" || typeof binding.queueLifecycleGeneration !== "number" || !Number.isSafeInteger(binding.queueLifecycleGeneration)) invalid();
				break;
			case "kv_namespace":
			case "r2_bucket":
			case "d1_database":
			case "do_namespace":
				if (typeof binding.resourceId !== "string" || typeof binding.resourceSpecGeneration !== "number" || !Number.isSafeInteger(binding.resourceSpecGeneration) || !record(binding.permissions) || typeof binding.permissions.read !== "boolean" || typeof binding.permissions.write !== "boolean" || binding.namespacePrefix !== undefined && typeof binding.namespacePrefix !== "string" || binding.namespaceNameKey !== undefined && typeof binding.namespaceNameKey !== "string") invalid();
				break;
			default: invalid();
		}
	}
}

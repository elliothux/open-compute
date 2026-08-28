// Generated from runtime/src/loader/wrappers/generator.ts by Rolldown. Do not edit.
/** Platform-owned module paths preserve the TypeScript dependency layout. */
export const INTERNAL_MODULE_PREFIX = "__open_compute__/";
export const R2_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}r2/facade.js`;
export const D1_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}d1/facade.js`;
export const DO_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/facade.js`;
export const DO_ID_CODEC_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/id-codec.js`;
export const DO_ALARM_SHIM_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/alarm-shim.js`;
export const QUEUE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}queues/facade.js`;
export const WORKFLOW_RUNNER_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/runner.js`;
export const WORKFLOW_RUNNER_V2_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/runner-v2.js`;
export const WORKFLOW_JSON_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/json.js`;
export const WORKFLOW_JSON_V2_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/json-v2.js`;
export const WORKFLOW_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/facade.js`;
export const WORKFLOW_V2_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/facade-v2.js`;
export const WRAPPER_RUNTIME_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/runtime.js`;
export const DO_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/durable-object.js`;
export const WORKFLOW_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/workflow.js`;
export const LOADED_ISOLATE_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}entry.js`;
export const VALIDATION_MODULE = `${INTERNAL_MODULE_PREFIX}validation.js`;
function fromWrapper(module) {
	return JSON.stringify(`./${module.slice(INTERNAL_MODULE_PREFIX.length)}`);
}
/** Only module wiring and validated data are generated; behavior lives in TS modules. */
export function generateBindingWrapper(options) {
	const { mainModule, bindings, entrypointName, durableObject, workflowCapability } = options;
	if (entrypointName !== undefined && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)) {
		throw new Error("invalid entrypoint name");
	}
	if ((workflowCapability !== undefined || durableObject) && entrypointName === undefined) throw new Error("missing entrypoint");
	const main = JSON.stringify(`../${mainModule}`);
	const lines = [
		`import * as tenant from ${main};`,
		`export * from ${main};`,
		`import { createEnvironment, wrapDefault, wrapEntrypoint } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};`
	];
	const factories = [];
	for (const [kind, version, module, exported] of [
		[
			"r2_bucket",
			1,
			R2_FACADE_MODULE,
			"R2Bucket"
		],
		[
			"d1_database",
			1,
			D1_FACADE_MODULE,
			"D1Database"
		],
		[
			"do_namespace",
			1,
			DO_FACADE_MODULE,
			"DurableObjectNamespace"
		],
		[
			"queue_producer",
			1,
			QUEUE_FACADE_MODULE,
			"QueueProducer"
		],
		[
			"workflow",
			1,
			WORKFLOW_FACADE_MODULE,
			"WorkflowBinding"
		],
		[
			"workflow",
			2,
			WORKFLOW_V2_FACADE_MODULE,
			"WorkflowBindingV2"
		]
	]) {
		const names = bindings.filter((binding) => binding.kind === kind && binding.capabilityVersion === version).map((binding) => binding.name);
		if (names.length === 0) continue;
		lines.push(`import { ${exported} } from ${fromWrapper(module)};`);
		factories.push(`{ names: ${JSON.stringify(names)}, create: ${exported} }`);
	}
	lines.push(`const wrapEnv = createEnvironment([${factories.join(",")}], ${durableObject});`);
	if (workflowCapability !== undefined) {
		lines.push(`import { createWorkflowEntrypoint } from ${fromWrapper(WORKFLOW_WRAPPER_MODULE)};`);
		lines.push(`import { runWorkflow, validateWorkflowClass } from ${fromWrapper(workflowCapability === 2 ? WORKFLOW_RUNNER_V2_MODULE : WORKFLOW_RUNNER_MODULE)};`);
		lines.push(`const __OpenComputeWorkflow = createWorkflowEntrypoint(tenant[${JSON.stringify(entrypointName)}], wrapEnv, runWorkflow, validateWorkflowClass);`);
		lines.push("export { __OpenComputeWorkflow };");
	} else if (entrypointName !== undefined && (durableObject || entrypointName !== "default")) {
		const factory = durableObject ? "wrapDurableObject" : "wrapEntrypoint";
		if (durableObject) lines.push(`import { wrapDurableObject } from ${fromWrapper(DO_WRAPPER_MODULE)};`);
		lines.push(`const NamedWrapped = ${factory}(tenant[${JSON.stringify(entrypointName)}], wrapEnv, ${JSON.stringify(entrypointName)});`);
		lines.push(`export { NamedWrapped as ${entrypointName} };`);
	}
	if (!(durableObject && entrypointName === "default")) lines.push("export default wrapDefault(tenant.default, wrapEnv);");
	return lines.join("\n");
}
export function generateValidationWrapper(entrypointName) {
	return `import * as tenant from "./entry.js";\nimport { validationHandler } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};\nexport default validationHandler(tenant, ${JSON.stringify(entrypointName ?? "default")});`;
}

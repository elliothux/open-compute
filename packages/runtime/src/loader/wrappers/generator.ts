import type { RuntimeBinding, RuntimeServiceBinding } from "../protocol.js";

/** Platform-owned module paths preserve the TypeScript dependency layout. */
export const INTERNAL_MODULE_PREFIX = "__open_compute__/";
export const R2_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}r2/facade.js`;
export const D1_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}d1/facade.js`;
export const DO_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/facade.js`;
export const DO_ID_CODEC_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/id-codec.js`;
export const DO_ALARM_SHIM_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/alarm-shim.js`;
export const QUEUE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}queues/facade.js`;
export const WORKFLOW_RUNNER_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/runner.js`;
export const WORKFLOW_JSON_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/json.js`;
export const WORKFLOW_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/facade.js`;
export const ASSET_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}assets/facade.js`;
export const SERVICE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}services/facade.js`;
export const SERVICE_SCOPE_MODULE = `${INTERNAL_MODULE_PREFIX}services/scope.js`;
export const WRAPPER_RUNTIME_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/runtime.js`;
export const DO_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/durable-object.js`;
export const WORKFLOW_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/workflow.js`;
export const LOADED_ISOLATE_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}entry.js`;
export const VALIDATION_MODULE = `${INTERNAL_MODULE_PREFIX}validation.js`;

export interface WrapperOptions {
  mainModule: string;
  bindings: readonly RuntimeBinding[];
  services: readonly RuntimeServiceBinding[];
  entrypointName?: string | undefined;
  durableObject: boolean;
  workflow?: boolean | undefined;
  assetBindingName?: string | undefined;
}

function fromWrapper(module: string): string { return JSON.stringify(`./${module.slice(INTERNAL_MODULE_PREFIX.length)}`); }

/** Only module wiring and validated data are generated; behavior lives in TS modules. */
export function generateBindingWrapper(options: WrapperOptions): string {
  const { mainModule, bindings, services, entrypointName, durableObject, workflow = false, assetBindingName } = options;
  if (entrypointName !== undefined && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)) {
    throw new Error("invalid entrypoint name");
  }
  if ((workflow || durableObject) && entrypointName === undefined) throw new Error("missing entrypoint");
  const main = JSON.stringify(`../${mainModule}`);
  const lines = [
    `import * as tenant from ${main};`, `export * from ${main};`,
    `import { createEnvironment, wrapDefault, wrapDefaultService, wrapEntrypoint } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};`,
  ];
  const factories: string[] = [];
  if (assetBindingName !== undefined) {
    lines.push(`import { AssetsBinding } from ${fromWrapper(ASSET_FACADE_MODULE)};`);
    factories.push(`{ names: ${JSON.stringify([assetBindingName])}, create: AssetsBinding }`);
  }
  if (services.length > 0) {
    lines.push(`import { ServiceBinding } from ${fromWrapper(SERVICE_FACADE_MODULE)};`);
    factories.push(`{ names: ${JSON.stringify(services.map(service => service.name))}, create: ServiceBinding }`);
  }
  for (const [kind, version, module, exported] of [
    ["r2_bucket", 1, R2_FACADE_MODULE, "R2Bucket"],
    ["d1_database", 1, D1_FACADE_MODULE, "D1Database"],
    ["do_namespace", 1, DO_FACADE_MODULE, "DurableObjectNamespace"],
    ["queue_producer", 1, QUEUE_FACADE_MODULE, "QueueProducer"],
    ["workflow", 1, WORKFLOW_FACADE_MODULE, "WorkflowBinding"],
  ] as const) {
    const names = bindings.filter(binding => binding.kind === kind && binding.capabilityVersion === version).map(binding => binding.name);
    if (names.length === 0) continue;
    lines.push(`import { ${exported} } from ${fromWrapper(module)};`);
    factories.push(`{ names: ${JSON.stringify(names)}, create: ${exported} }`);
  }
  lines.push(`const wrapEnv = createEnvironment([${factories.join(",")}], ${durableObject});`);
  if (workflow) {
    lines.push(`import { createWorkflowEntrypoint } from ${fromWrapper(WORKFLOW_WRAPPER_MODULE)};`);
    lines.push(`import { runWorkflow, validateWorkflowClass } from ${fromWrapper(WORKFLOW_RUNNER_MODULE)};`);
    lines.push(`const __OpenComputeWorkflow = createWorkflowEntrypoint(tenant[${JSON.stringify(entrypointName)}], wrapEnv, runWorkflow, validateWorkflowClass);`);
    lines.push("export { __OpenComputeWorkflow };");
  } else if (entrypointName !== undefined && (durableObject || entrypointName !== "default")) {
    const factory = durableObject ? "wrapDurableObject" : "wrapEntrypoint";
    if (durableObject) lines.push(`import { wrapDurableObject } from ${fromWrapper(DO_WRAPPER_MODULE)};`);
    lines.push(`const NamedWrapped = ${factory}(tenant[${JSON.stringify(entrypointName)}], wrapEnv, ${JSON.stringify(entrypointName)});`);
    lines.push(`export { NamedWrapped as ${entrypointName} };`);
  }
  if (!durableObject && !workflow) {
    lines.push("const __OpenComputeDefaultService = wrapDefaultService(tenant.default, wrapEnv);");
    lines.push("export { __OpenComputeDefaultService };");
  }
  if (!(durableObject && entrypointName === "default")) lines.push("export default wrapDefault(tenant.default, wrapEnv);");
  return lines.join("\n");
}

export function generateValidationWrapper(entrypointName: string | undefined): string {
  return `import * as tenant from "./entry.js";\nimport { validationHandler } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};\nexport default validationHandler(tenant, ${JSON.stringify(entrypointName ?? "default")});`;
}

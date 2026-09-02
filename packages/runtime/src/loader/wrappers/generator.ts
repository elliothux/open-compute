import type { RuntimeBinding, RuntimeScheduledTarget, RuntimeServiceBinding } from "../protocol.js";

/** Platform-owned module paths preserve the TypeScript dependency layout. */
export const INTERNAL_MODULE_PREFIX = "__open_compute__/";
export const KV_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}kv/facade.js`;
export const R2_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}r2/facade.js`;
export const D1_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}d1/facade.js`;
export const DO_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/facade.js`;
export const DO_ID_CODEC_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/id-codec.js`;
export const DO_ALARM_SHIM_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/alarm-shim.js`;
export const DO_OUTPUT_GATE_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/output-gate.js`;
export const DO_FACETS_MODULE = `${INTERNAL_MODULE_PREFIX}durable-objects/facets.js`;
export const QUEUE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}queues/facade.js`;
export const SERIALIZATION_CODEC_MODULE = `${INTERNAL_MODULE_PREFIX}serialization/codec.js`;
export const SERIALIZATION_ENCODE_MODULE = `${INTERNAL_MODULE_PREFIX}serialization/encode.js`;
export const SERIALIZATION_DECODE_MODULE = `${INTERNAL_MODULE_PREFIX}serialization/decode.js`;
export const SERIALIZATION_FORMAT_MODULE = `${INTERNAL_MODULE_PREFIX}serialization/format.js`;
export const WORKFLOW_RUNNER_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/runner.js`;
export const WORKFLOW_DURATION_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/duration.js`;
export const WORKFLOW_CODEC_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/codec.js`;
export const WORKFLOW_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}workflows/facade.js`;
export const ASSET_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}assets/facade.js`;
export const SERVICE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}services/facade.js`;
export const SERVICE_SCOPE_MODULE = `${INTERNAL_MODULE_PREFIX}services/scope.js`;
export const SOCKET_TUNNEL_MODULE = `${INTERNAL_MODULE_PREFIX}sockets/tunnel.js`;
export const CACHE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}cache/facade.js`;
export const IMAGES_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}images/facade.js`;
export const AI_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}ai/facade.js`;
export const VECTORIZE_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}vectorize/facade.js`;
export const AI_SEARCH_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}ai-search/facade.js`;
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
  imagesBindingName?: string | undefined;
  aiBindingName?: string | undefined;
  cacheAvailable: boolean;
  automaticCacheEnabled: boolean;
  cacheFailOpen: boolean;
  automaticCacheEntrypoints?: readonly string[] | undefined;
  scheduledTargets?: readonly RuntimeScheduledTarget[] | undefined;
}

function fromWrapper(module: string): string { return JSON.stringify(`./${module.slice(INTERNAL_MODULE_PREFIX.length)}`); }

/** Only module wiring and validated data are generated; behavior lives in TS modules. */
export function generateBindingWrapper(options: WrapperOptions): string {
  const { mainModule, bindings, services, entrypointName, durableObject, workflow = false,
    assetBindingName, imagesBindingName, aiBindingName, cacheAvailable, automaticCacheEnabled,
    cacheFailOpen, automaticCacheEntrypoints = [], scheduledTargets = [] } = options;
  if (entrypointName !== undefined && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)) {
    throw new Error("invalid entrypoint name");
  }
  if ((workflow || durableObject) && entrypointName === undefined) throw new Error("missing entrypoint");
  const workflowBindings = new Map(bindings
    .filter(binding => binding.kind === "workflow")
    .map(binding => [binding.name, binding]));
  if (scheduledTargets.length > 100
      || scheduledTargets.some((target, index) => typeof target.cron !== "string"
        || target.cron.length < 1 || target.cron.length > 256
        || typeof target.scheduledHandler !== "boolean"
        || !Array.isArray(target.workflowBindings) || target.workflowBindings.length > 100
        || (!target.scheduledHandler && target.workflowBindings.length === 0)
        || (index > 0 && scheduledTargets[index - 1]!.cron >= target.cron)
        || target.workflowBindings.some((name, bindingIndex) => !/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name)
          || name.startsWith("OPEN_COMPUTE_") || name.startsWith("__")
          || (bindingIndex > 0 && target.workflowBindings[bindingIndex - 1]! >= name)
          || workflowBindings.get(name)?.schedules?.includes(target.cron) !== true))
      || [...workflowBindings.values()].some(binding => binding.schedules?.some(cron =>
        !scheduledTargets.some(target => target.cron === cron
          && target.workflowBindings.includes(binding.name))))) {
    throw new Error("invalid scheduled targets");
  }
  const main = JSON.stringify(`../${mainModule}`);
  const lines: string[] = [];
  if (cacheAvailable) {
    lines.push(`import { createCacheRuntime } from ${fromWrapper(CACHE_FACADE_MODULE)};`);
  }
  lines.push(
    `import * as tenant from ${main};`, `export * from ${main};`,
    `import { createEnvironment, wrapDefault, wrapDefaultService, wrapEntrypoint } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};`,
  );
  const factories: string[] = [];
  if (assetBindingName !== undefined) {
    lines.push(`import { AssetsBinding } from ${fromWrapper(ASSET_FACADE_MODULE)};`);
    factories.push(`{ names: ${JSON.stringify([assetBindingName])}, create: AssetsBinding }`);
  }
  if (services.length > 0) {
    lines.push(`import { ServiceBinding } from ${fromWrapper(SERVICE_FACADE_MODULE)};`);
    factories.push(`{ names: ${JSON.stringify(services.map(service => service.name))}, create: ServiceBinding }`);
  }
  if (imagesBindingName !== undefined) {
    lines.push(`import { ImagesBinding } from ${fromWrapper(IMAGES_FACADE_MODULE)};`);
    factories.push(`{ names: ${JSON.stringify([imagesBindingName])}, create: ImagesBinding }`);
  }
  if (aiBindingName !== undefined) {
    lines.push(`import { AiBinding } from ${fromWrapper(AI_FACADE_MODULE)};`);
    factories.push(`{ names: ${JSON.stringify([aiBindingName])}, create: AiBinding }`);
  }
  for (const [kind, version, module, exported] of [
    ["kv_namespace", 1, KV_FACADE_MODULE, "KVNamespace"],
    ["r2_bucket", 1, R2_FACADE_MODULE, "R2Bucket"],
    ["d1_database", 1, D1_FACADE_MODULE, "D1Database"],
    ["do_namespace", 1, DO_FACADE_MODULE, "DurableObjectNamespace"],
    ["queue_producer", 1, QUEUE_FACADE_MODULE, "QueueProducer"],
    ["workflow", 1, WORKFLOW_FACADE_MODULE, "WorkflowBinding"],
    ["vectorize_index", 1, VECTORIZE_FACADE_MODULE, "VectorizeBinding"],
    ["ai_search_namespace", 1, AI_SEARCH_FACADE_MODULE, "AiSearchNamespaceBinding"],
    ["ai_search_instance", 1, AI_SEARCH_FACADE_MODULE, "AiSearchInstanceBinding"],
  ] as const) {
    const names = bindings.filter(binding => binding.kind === kind && binding.capabilityVersion === version).map(binding => binding.name);
    if (names.length === 0) continue;
    lines.push(`import { ${exported} } from ${fromWrapper(module)};`);
    factories.push(`{ names: ${JSON.stringify(names)}, create: ${exported} }`);
  }
  lines.push(`const wrapEnv = createEnvironment([${factories.join(",")}], ${durableObject});`);
  lines.push(`const cacheRuntime = ${cacheAvailable ? `createCacheRuntime(${automaticCacheEnabled}, ${cacheFailOpen}, ${JSON.stringify(entrypointName ?? "default")})` : "undefined"};`);
  if (workflow) {
    lines.push(`import { createWorkflowEntrypoint } from ${fromWrapper(WORKFLOW_WRAPPER_MODULE)};`);
    lines.push(`import { runWorkflow, validateWorkflowClass } from ${fromWrapper(WORKFLOW_RUNNER_MODULE)};`);
    lines.push(`const __OpenComputeWorkflow = createWorkflowEntrypoint(tenant[${JSON.stringify(entrypointName)}], wrapEnv, runWorkflow, validateWorkflowClass);`);
    lines.push("export { __OpenComputeWorkflow };");
  } else if (entrypointName !== undefined && (durableObject || entrypointName !== "default")) {
    const factory = durableObject ? "wrapDurableObject" : "wrapEntrypoint";
    if (durableObject) lines.push(`import { wrapDurableObject } from ${fromWrapper(DO_WRAPPER_MODULE)};`);
    lines.push(`const NamedWrapped = ${factory}(tenant[${JSON.stringify(entrypointName)}], wrapEnv, ${JSON.stringify(entrypointName)}, cacheRuntime);`);
    lines.push(`export { NamedWrapped as ${entrypointName} };`);
  } else if (entrypointName === undefined) {
    for (const [index, name] of automaticCacheEntrypoints.entries()) {
      if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(name) || name === "default") {
        throw new Error("invalid entrypoint name");
      }
      const local = `__OpenComputeCachedEntrypoint${index}`;
      lines.push(`const ${local} = wrapEntrypoint(tenant[${JSON.stringify(name)}], wrapEnv, ${JSON.stringify(name)}, createCacheRuntime(true, ${cacheFailOpen}, ${JSON.stringify(name)}));`);
      lines.push(`export { ${local} as ${name} };`);
    }
  }
  if (!durableObject && !workflow) {
    lines.push("const __OpenComputeDefaultService = wrapDefaultService(tenant.default, wrapEnv, cacheRuntime);");
    lines.push("export { __OpenComputeDefaultService };");
  }
  if (!(durableObject && entrypointName === "default")) {
    const scheduledWorkflows = scheduledTargets.some(target => target.workflowBindings.length > 0);
    if (scheduledWorkflows) {
      lines.push(`import { triggerWorkflowSchedule } from ${fromWrapper(WORKFLOW_FACADE_MODULE)};`);
    }
    lines.push(`export default wrapDefault(tenant.default, wrapEnv, cacheRuntime, ${scheduledWorkflows ? `{ targets: ${JSON.stringify(scheduledTargets)}, trigger: triggerWorkflowSchedule }` : "undefined"});`);
  }
  return lines.join("\n");
}

export function generateValidationWrapper(entrypointName: string | undefined): string {
  return `import * as tenant from "./entry.js";\nimport { validationHandler } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};\nexport default validationHandler(tenant, ${JSON.stringify(entrypointName ?? "default")});`;
}

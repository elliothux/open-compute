import type {
  RuntimeBinding,
  RuntimeScheduledTarget,
  RuntimeServiceBinding,
} from "../protocol.js";

/** Platform-owned module paths preserve the TypeScript dependency layout. */
export const INTERNAL_MODULE_PREFIX = "__open_compute__/";
export const KV_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}kv/facade.js`;
export const R2_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}r2/facade.js`;
export const R2_VALIDATION_MODULE = `${INTERNAL_MODULE_PREFIX}r2/validation.js`;
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
export const AI_SEARCH_RESPONSES_MODULE = `${INTERNAL_MODULE_PREFIX}ai-search/responses.js`;
export const AI_SEARCH_VALIDATION_MODULE = `${INTERNAL_MODULE_PREFIX}ai-search/validation.js`;
export const ARTIFACTS_FACADE_MODULE = `${INTERNAL_MODULE_PREFIX}artifacts/facade.js`;
export const LOOPBACK_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/loopback.js`;
export const WRAPPER_RUNTIME_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/runtime.js`;
export const WRAPPER_COMPLETION_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/completion.js`;
export const DO_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/durable-object.js`;
export const WORKFLOW_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/workflow.js`;
export const LOADED_ISOLATE_WRAPPER_MODULE = `${INTERNAL_MODULE_PREFIX}entry.js`;
export const VALIDATION_MODULE = `${INTERNAL_MODULE_PREFIX}validation.js`;
export const FORWARDING_SOURCES_MODULE = `${INTERNAL_MODULE_PREFIX}loader/forwarding-sources.js`;
export const FORWARDING_MODULE = `${INTERNAL_MODULE_PREFIX}loader/forwarding.js`;
export const GENERATOR_MODULE = `${INTERNAL_MODULE_PREFIX}loader/wrappers/generator.js`;
export const OPEN_COMPUTE_FORWARDING_MODULE = "open-compute:worker-loader";
export const PRIVATE_WEAK_MAP_MODULE = `${INTERNAL_MODULE_PREFIX}private-weak-map.js`;

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
  automaticCacheEnabled: boolean;
  cacheFailOpen: boolean;
  cacheTransportAvailable?: boolean | undefined;
  automaticCacheEntrypoints?: readonly string[] | undefined;
  scheduledTargets?: readonly RuntimeScheduledTarget[] | undefined;
  sourceIdentity?: string | undefined;
  workerLoaderNames?: readonly string[] | undefined;
  forwardedChild?: boolean | undefined;
}

function fromWrapper(module: string): string {
  return safeStringify(
    `./${safeApply(stringSlice, module, [INTERNAL_MODULE_PREFIX.length])}`,
  );
}

const safeStringify = JSON.stringify;
const safeApply = Reflect.apply;
const stringSlice = String.prototype.slice;
const arrayJoin = Array.prototype.join;

function quotedNames(names: readonly string[]): string {
  const quoted: string[] = [];
  for (let index = 0; index < names.length; index++)
    quoted[quoted.length] = safeStringify(names[index]!);
  return `[${safeApply(arrayJoin, quoted, [","])}]`;
}

function forwardedChildWrapper(options: WrapperOptions): string {
  const imports = [
    `import { createCacheRuntime } from ${fromWrapper(CACHE_FACADE_MODULE)};`,
    `import { createLoopbackEntrypoint } from ${fromWrapper(LOOPBACK_MODULE)};`,
    `import { createEnvironment, wrapDefault, wrapDefaultService, wrapEntrypoint } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};`,
  ];
  const factories: string[] = [];
  const extras = [
    [options.assetBindingName, ASSET_FACADE_MODULE, "AssetsBinding"],
    [options.imagesBindingName, IMAGES_FACADE_MODULE, "ImagesBinding"],
    [options.aiBindingName, AI_FACADE_MODULE, "AiBinding"],
  ] as const;
  for (let index = 0; index < extras.length; index++) {
    const extra = extras[index]!;
    const name = extra[0];
    const module = extra[1];
    const exported = extra[2];
    if (name === undefined) continue;
    imports[imports.length] =
      `import { ${exported} } from ${fromWrapper(module)};`;
    factories[factories.length] =
      `{ names: ${quotedNames([name])}, create: ${exported} }`;
  }
  if (options.services.length > 0) {
    const names: string[] = [];
    for (let index = 0; index < options.services.length; index++)
      names[names.length] = options.services[index]!.name;
    imports[imports.length] =
      `import { ServiceBinding } from ${fromWrapper(SERVICE_FACADE_MODULE)};`;
    factories[factories.length] =
      `{ names: ${quotedNames(names)}, create: ServiceBinding }`;
  }
  const kinds = [
    ["kv_namespace", KV_FACADE_MODULE, "KVNamespace"],
    ["r2_bucket", R2_FACADE_MODULE, "R2Bucket"],
    ["d1_database", D1_FACADE_MODULE, "D1Database"],
    ["do_namespace", DO_FACADE_MODULE, "DurableObjectNamespace"],
    ["queue_producer", QUEUE_FACADE_MODULE, "QueueProducer"],
    ["workflow", WORKFLOW_FACADE_MODULE, "WorkflowBinding"],
    ["vectorize_index", VECTORIZE_FACADE_MODULE, "VectorizeBinding"],
    [
      "ai_search_namespace",
      AI_SEARCH_FACADE_MODULE,
      "AiSearchNamespaceBinding",
    ],
    ["ai_search_instance", AI_SEARCH_FACADE_MODULE, "AiSearchInstanceBinding"],
    ["artifacts_namespace", ARTIFACTS_FACADE_MODULE, "ArtifactsBinding"],
  ] as const;
  for (let kindIndex = 0; kindIndex < kinds.length; kindIndex++) {
    const entry = kinds[kindIndex]!;
    const kind = entry[0];
    const module = entry[1];
    const exported = entry[2];
    const names: string[] = [];
    for (
      let bindingIndex = 0;
      bindingIndex < options.bindings.length;
      bindingIndex++
    ) {
      const binding = options.bindings[bindingIndex]!;
      if (binding.kind === kind && binding.capabilityVersion === 1)
        names[names.length] = binding.name;
    }
    if (names.length === 0) continue;
    imports[imports.length] =
      `import { ${exported} } from ${fromWrapper(module)};`;
    factories[factories.length] =
      `{ names: ${quotedNames(names)}, create: ${exported} }`;
  }
  imports[imports.length] =
    `import * as tenant from ${safeStringify(`../${options.mainModule}`)};`;
  imports[imports.length] =
    `const wrapEnv = createEnvironment([${safeApply(arrayJoin, factories, [","])}], false);`;
  imports[imports.length] =
    `export const __OpenComputeLoopbackService = createLoopbackEntrypoint(tenant, wrapEnv, wrapEntrypoint, []);`;
  imports[imports.length] =
    `const cacheRuntime = createCacheRuntime(false, false, "default", false);`;
  imports[imports.length] =
    `export const __OpenComputeDefaultService = wrapDefaultService(tenant.default, wrapEnv, cacheRuntime);`;
  imports[imports.length] =
    `export default wrapDefault(tenant.default, wrapEnv, cacheRuntime, undefined);`;
  return safeApply(arrayJoin, imports, ["\n"]);
}

/** Only module wiring and validated data are generated; behavior lives in TS modules. */
export function generateBindingWrapper(options: WrapperOptions): string {
  if (options.forwardedChild) return forwardedChildWrapper(options);
  const {
    mainModule,
    bindings,
    services,
    entrypointName,
    durableObject,
    workflow = false,
    assetBindingName,
    imagesBindingName,
    aiBindingName,
    automaticCacheEnabled,
    cacheFailOpen,
    cacheTransportAvailable = true,
    automaticCacheEntrypoints = [],
    scheduledTargets = [],
    sourceIdentity,
    workerLoaderNames,
    forwardedChild = false,
  } = options;
  if (
    entrypointName !== undefined &&
    !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)
  ) {
    throw new Error("invalid entrypoint name");
  }
  if ((workflow || durableObject) && entrypointName === undefined)
    throw new Error("missing entrypoint");
  const workflowBindings = new Map(
    bindings
      .filter((binding) => binding.kind === "workflow")
      .map((binding) => [binding.name, binding]),
  );
  if (
    scheduledTargets.length > 100 ||
    scheduledTargets.some(
      (target, index) =>
        typeof target.cron !== "string" ||
        target.cron.length < 1 ||
        target.cron.length > 256 ||
        typeof target.scheduledHandler !== "boolean" ||
        !Array.isArray(target.workflowBindings) ||
        target.workflowBindings.length > 100 ||
        (!target.scheduledHandler && target.workflowBindings.length === 0) ||
        (index > 0 && scheduledTargets[index - 1]!.cron >= target.cron) ||
        target.workflowBindings.some(
          (name, bindingIndex) =>
            !/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name) ||
            name.startsWith("OPEN_COMPUTE_") ||
            name.startsWith("__") ||
            (bindingIndex > 0 &&
              target.workflowBindings[bindingIndex - 1]! >= name) ||
            workflowBindings.get(name)?.schedules?.includes(target.cron) !==
              true,
        ),
    ) ||
    [...workflowBindings.values()].some((binding) =>
      binding.schedules?.some(
        (cron) =>
          !scheduledTargets.some(
            (target) =>
              target.cron === cron &&
              target.workflowBindings.includes(binding.name),
          ),
      ),
    )
  ) {
    throw new Error("invalid scheduled targets");
  }
  const main = JSON.stringify(`../${mainModule}`);
  const lines: string[] = [];
  if (sourceIdentity !== undefined) {
    lines.push(
      `import { createForwarding, newWeakRegistry, registryGet, registrySet, newDescriptorRegistry, descriptorGet, descriptorSet, nativeLoader } from ${fromWrapper(FORWARDING_MODULE)};`,
      `import { generateBindingWrapper } from ${fromWrapper(GENERATOR_MODULE)};`,
      `import sources from ${fromWrapper(FORWARDING_SOURCES_MODULE)};`,
    );
  }
  lines.push(
    `import { createCacheRuntime } from ${fromWrapper(CACHE_FACADE_MODULE)};`,
  );
  lines.push(
    `import { createLoopbackEntrypoint } from ${fromWrapper(LOOPBACK_MODULE)};`,
    `import { createEnvironment, wrapDefault, wrapDefaultService, wrapEntrypoint } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};`,
  );
  const factories: string[] = [];
  if (assetBindingName !== undefined) {
    lines.push(
      `import { AssetsBinding } from ${fromWrapper(ASSET_FACADE_MODULE)};`,
    );
    factories.push(
      `{ names: ${JSON.stringify([assetBindingName])}, create: AssetsBinding }`,
    );
  }
  if (services.length > 0) {
    lines.push(
      `import { ServiceBinding } from ${fromWrapper(SERVICE_FACADE_MODULE)};`,
    );
    factories.push(
      `{ names: ${JSON.stringify(services.map((service) => service.name))}, create: ServiceBinding }`,
    );
  }
  if (imagesBindingName !== undefined) {
    lines.push(
      `import { ImagesBinding } from ${fromWrapper(IMAGES_FACADE_MODULE)};`,
    );
    factories.push(
      `{ names: ${JSON.stringify([imagesBindingName])}, create: ImagesBinding }`,
    );
  }
  if (aiBindingName !== undefined) {
    lines.push(`import { AiBinding } from ${fromWrapper(AI_FACADE_MODULE)};`);
    factories.push(
      `{ names: ${JSON.stringify([aiBindingName])}, create: AiBinding }`,
    );
  }
  for (const [kind, version, module, exported] of [
    ["kv_namespace", 1, KV_FACADE_MODULE, "KVNamespace"],
    ["r2_bucket", 1, R2_FACADE_MODULE, "R2Bucket"],
    ["d1_database", 1, D1_FACADE_MODULE, "D1Database"],
    ["do_namespace", 1, DO_FACADE_MODULE, "DurableObjectNamespace"],
    ["queue_producer", 1, QUEUE_FACADE_MODULE, "QueueProducer"],
    ["workflow", 1, WORKFLOW_FACADE_MODULE, "WorkflowBinding"],
    ["vectorize_index", 1, VECTORIZE_FACADE_MODULE, "VectorizeBinding"],
    [
      "ai_search_namespace",
      1,
      AI_SEARCH_FACADE_MODULE,
      "AiSearchNamespaceBinding",
    ],
    [
      "ai_search_instance",
      1,
      AI_SEARCH_FACADE_MODULE,
      "AiSearchInstanceBinding",
    ],
    ["artifacts_namespace", 1, ARTIFACTS_FACADE_MODULE, "ArtifactsBinding"],
  ] as const) {
    const names = bindings
      .filter(
        (binding) =>
          binding.kind === kind && binding.capabilityVersion === version,
      )
      .map((binding) => binding.name);
    if (names.length === 0) continue;
    lines.push(`import { ${exported} } from ${fromWrapper(module)};`);
    factories.push(`{ names: ${JSON.stringify(names)}, create: ${exported} }`);
  }
  if (workflow) {
    lines.push(
      `import { createWorkflowEntrypoint } from ${fromWrapper(WORKFLOW_WRAPPER_MODULE)};`,
      `import { runWorkflow, validateWorkflowClass } from ${fromWrapper(WORKFLOW_RUNNER_MODULE)};`,
    );
  } else if (durableObject) {
    lines.push(
      `import { wrapDurableObject } from ${fromWrapper(DO_WRAPPER_MODULE)};`,
    );
  }
  const scheduledWorkflows = scheduledTargets.some(
    (target) => target.workflowBindings.length > 0,
  );
  if (scheduledWorkflows)
    lines.push(
      `import { triggerWorkflowSchedule } from ${fromWrapper(WORKFLOW_FACADE_MODULE)};`,
    );
  lines.push(`import * as tenant from ${main};`);
  if (!forwardedChild) lines.push(`export * from ${main};`);
  if (sourceIdentity !== undefined) {
    if (!workerLoaderNames?.length)
      throw new Error("missing forwarding loader");
    const forwardingEntries = [
      ...bindings.map((binding) => [
        binding.name,
        { kind: "binding", descriptor: binding },
      ]),
      ...services.map((service) => [
        service.name,
        { kind: "service", descriptor: service },
      ]),
      ...(assetBindingName === undefined
        ? []
        : [[assetBindingName, { kind: "assets" }]]),
      ...(imagesBindingName === undefined
        ? []
        : [[imagesBindingName, { kind: "images" }]]),
      ...(aiBindingName === undefined ? [] : [[aiBindingName, { kind: "ai" }]]),
    ];
    lines.push(
      `const { getWorker: forwardGet, loadWorker: forwardLoad } = createForwarding(generateBindingWrapper, ${JSON.stringify(INTERNAL_MODULE_PREFIX)}, ${JSON.stringify(LOADED_ISOLATE_WRAPPER_MODULE)}, nativeLoader);`,
      `const forwardingRoots = newWeakRegistry();`,
      `const forwardingLoaders = newWeakRegistry();`,
      `const forwardingDescriptors = newDescriptorRegistry();`,
      `const descriptorEntries = ${JSON.stringify(forwardingEntries)};`,
      `for (let index = 0; index < descriptorEntries.length; index++) descriptorSet(forwardingDescriptors, descriptorEntries[index][0], descriptorEntries[index][1]);`,
      `const createWrappedEnv = createEnvironment([${factories.join(",")}], ${durableObject}, (name, facade, transport, rawEnv) => {`,
      `  const root = descriptorGet(forwardingDescriptors, name);`,
      `  const owner = rawEnv.__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS;`,
      `  if (root && owner) registrySet(forwardingRoots, facade, { ...root, transport, owner });`,
      `});`,
      `const wrapEnv = (rawEnv) => {`,
      `  const wrapped = createWrappedEnv(rawEnv);`,
      `  const owner = rawEnv.__OPEN_COMPUTE_PRIVATE_FORWARDING_LOADERS;`,
      `  const loaderNames = ${JSON.stringify(workerLoaderNames)};`,
      `  if (owner) for (let index = 0; index < loaderNames.length; index++) {`,
      `    const name = loaderNames[index];`,
      `    if (wrapped[name] && owner[name]) registrySet(forwardingLoaders, wrapped[name], { loader: owner[name], owner });`,
      `  }`,
      `  return wrapped;`,
      `};`,
      `const privateLoader = (loader) => {`,
      `  const granted = registryGet(forwardingLoaders, loader);`,
      `  if (!granted) throw new TypeError("WORKER_LOADER_FORWARDING_DENIED");`,
      `  return granted;`,
      `};`,
      `export const __OpenComputeGetWorker = (loader, id, callback) => { const grant = privateLoader(loader); return forwardGet(grant.loader, id, callback, forwardingRoots, sources, ${JSON.stringify(sourceIdentity)}, grant.owner); };`,
      `export const __OpenComputeLoadWorker = (loader, code) => { const grant = privateLoader(loader); return forwardLoad(grant.loader, code, forwardingRoots, sources, grant.owner); };`,
    );
  } else {
    lines.push(
      `const wrapEnv = createEnvironment([${factories.join(",")}], ${durableObject});`,
    );
  }
  lines.push(
    `export const __OpenComputeLoopbackService = createLoopbackEntrypoint(tenant, wrapEnv, wrapEntrypoint, ${JSON.stringify([...automaticCacheEntrypoints, ...(entrypointName && entrypointName !== "default" ? [entrypointName] : [])])});`,
  );
  lines.push(
    `const cacheRuntime = createCacheRuntime(${!durableObject && !workflow && automaticCacheEnabled}, ${cacheFailOpen}, ${JSON.stringify(entrypointName ?? "default")}${cacheTransportAvailable ? "" : ", false"});`,
  );
  if (workflow) {
    lines.push(
      `const __OpenComputeWorkflow = createWorkflowEntrypoint(tenant[${JSON.stringify(entrypointName)}], wrapEnv, runWorkflow, validateWorkflowClass, cacheRuntime);`,
    );
    lines.push("export { __OpenComputeWorkflow };");
  } else if (
    entrypointName !== undefined &&
    (durableObject || entrypointName !== "default")
  ) {
    const factory = durableObject ? "wrapDurableObject" : "wrapEntrypoint";
    lines.push(
      `const NamedWrapped = ${factory}(tenant[${JSON.stringify(entrypointName)}], wrapEnv, ${JSON.stringify(entrypointName)}, cacheRuntime);`,
    );
    lines.push(`export { NamedWrapped as ${entrypointName} };`);
  } else if (entrypointName === undefined) {
    for (const [index, name] of automaticCacheEntrypoints.entries()) {
      if (
        !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(name) ||
        name === "default"
      ) {
        throw new Error("invalid entrypoint name");
      }
      const local = `__OpenComputeCachedEntrypoint${index}`;
      lines.push(
        `const ${local} = wrapEntrypoint(tenant[${JSON.stringify(name)}], wrapEnv, ${JSON.stringify(name)}, createCacheRuntime(true, ${cacheFailOpen}, ${JSON.stringify(name)}));`,
      );
      lines.push(`export { ${local} as ${name} };`);
    }
  }
  if (!durableObject && !workflow) {
    lines.push(
      "const __OpenComputeDefaultService = wrapDefaultService(tenant.default, wrapEnv, cacheRuntime);",
    );
    lines.push("export { __OpenComputeDefaultService };");
  }
  if (!(durableObject && entrypointName === "default")) {
    lines.push(
      `export default wrapDefault(tenant.default, wrapEnv, cacheRuntime, ${scheduledWorkflows ? `{ targets: ${JSON.stringify(scheduledTargets)}, trigger: triggerWorkflowSchedule }` : "undefined"});`,
    );
  }
  return lines.join("\n");
}

export function generateValidationWrapper(
  entrypointName: string | undefined,
): string {
  return `import * as tenant from "./entry.js";\nimport { validationHandler } from ${fromWrapper(WRAPPER_RUNTIME_MODULE)};\nexport default validationHandler(tenant, ${JSON.stringify(entrypointName ?? "default")});`;
}

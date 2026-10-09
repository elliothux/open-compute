import type {
  RuntimeBinding,
  RuntimeScheduledTarget,
  RuntimeServiceBinding,
  RuntimeSnapshot,
} from "./protocol.js";

/** Private initialization data from a verified deployment or an explicit forwarding grant. */
export interface WorkerPolicy {
  readonly validation: boolean;
  readonly durableObject: boolean;
  readonly workflow: boolean;
  readonly entrypointName?: string | undefined;
  readonly bindings: readonly RuntimeBinding[];
  readonly services: readonly RuntimeServiceBinding[];
  readonly scheduledTargets: readonly RuntimeScheduledTarget[];
  readonly automaticCacheEnabled: boolean;
  readonly cacheFailOpen: boolean;
  readonly automaticCacheEntrypoints: readonly string[];
  readonly workerLoaderNames: readonly string[];
  readonly browserBindingNames: readonly string[];
  readonly sourceIdentity?: string | undefined;
  readonly assetBindingName?: string | undefined;
  readonly imagesBindingName?: string | undefined;
  readonly aiBindingName?: string | undefined;
}
export const PRIVATE_POLICY = "__OPEN_COMPUTE_PRIVATE_POLICY";

/** Select the common host policy without changing the tenant module graph. */
export function workerPolicy(
  snapshot: RuntimeSnapshot,
  validation: boolean,
  entrypointName: string | undefined,
  durableObject: boolean,
  workflow: boolean,
): WorkerPolicy {
  if (
    (entrypointName !== undefined &&
      !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)) ||
    ((durableObject || workflow) && entrypointName === undefined)
  )
    throw new Error("invalid entrypoint name");
  const scheduledTargets = snapshot.scheduledTargets;
  const workflowBindings = new Map(
    snapshot.bindings
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
  return {
    validation,
    durableObject,
    workflow,
    entrypointName,
    bindings: snapshot.bindings,
    services: snapshot.services,
    scheduledTargets,
    automaticCacheEnabled:
      !validation &&
      !durableObject &&
      !workflow &&
      (entrypointName === undefined
        ? snapshot.cachePolicy.enabled
        : (snapshot.cachePolicy.entrypoints[entrypointName]?.enabled ??
          snapshot.cachePolicy.enabled)),
    cacheFailOpen: snapshot.cachePolicy.failOpen,
    automaticCacheEntrypoints:
      !validation && entrypointName === undefined
        ? Object.entries(snapshot.cachePolicy.entrypoints)
            .filter(([, selected]) => selected.enabled)
            .map(([name]) => name)
        : [],
    workerLoaderNames: snapshot.workerLoaders.map((binding) => binding.name),
    sourceIdentity:
      !validation && snapshot.workerLoaders.length > 0
        ? `${snapshot.loaderKey}/${snapshot.routeGeneration}/${snapshot.workerCodeSha256}`
        : undefined,
    assetBindingName: snapshot.assetBinding?.name,
    imagesBindingName: snapshot.imagesBinding?.name,
    browserBindingNames: snapshot.browserBindings.map(
      (binding) => binding.name,
    ),
    aiBindingName: snapshot.aiBinding?.name,
  };
}

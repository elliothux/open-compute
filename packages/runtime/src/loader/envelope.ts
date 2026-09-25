import type { DispatchEnvelope } from "./protocol.js";

/** Validate the private dispatch identity before resolving any tenant source. */
export function assertEnvelope(
  request: Request,
  validation: boolean,
  entrypointName: string | undefined,
): DispatchEnvelope {
  const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
  const expected =
    request.headers.get("x-open-compute-worker-code-sha256") || "";
  const parts = loaderKey.split("/");
  const [instanceId, workerId, versionId] = parts;
  if (
    parts.length !== 3 ||
    !/^[0-9a-f]{32}$/.test(instanceId ?? "") ||
    [workerId, versionId].some(
      (part) =>
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
          part ?? "",
        ),
    ) ||
    request.headers.get("x-open-compute-instance-id") !== instanceId
  ) {
    throw new Error("invalid loader key");
  }
  if (!/^[0-9a-f]{64}$/.test(expected))
    throw new Error("invalid descriptor hash");
  if (
    entrypointName &&
    !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)
  ) {
    throw new Error("invalid entrypoint");
  }
  const routeGeneration = Number(
    request.headers.get("x-open-compute-route-generation"),
  );
  if (
    !Number.isSafeInteger(routeGeneration) ||
    (validation ? routeGeneration < 0 : routeGeneration < 1)
  ) {
    throw new Error("invalid route generation");
  }
  return {
    loaderKey,
    expected,
    routeGeneration,
    runtimeKey: `${validation ? "validate" : "runtime"}/${loaderKey}/${expected}/${routeGeneration}/${entrypointName || "default"}`,
  };
}

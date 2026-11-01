import { tenantEnv } from "./bindings.js";
import { assertEnvelope } from "./envelope.js";
import { modulesFor } from "./modules.js";
import type { LoaderEnv } from "./protocol.js";
import {
  bindingError,
  doPolicy,
  pythonPreparationCode,
  resolveSnapshot,
  stableCode,
  systemRequestId,
  tenantGlobalOutbound,
} from "./shared.js";

/** Return snapshot bytes only to the authenticated, bounded host preparation caller. */
export async function preparePython(
  request: Request,
  env: LoaderEnv,
  ctx: ExecutionContext,
): Promise<Response> {
  try {
    if (
      request.method !== "POST" ||
      request.headers.has("x-open-compute-entrypoint")
    )
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    const envelope = assertEnvelope(request, true, undefined);
    const snapshot = await resolveSnapshot(env, envelope, "preparation");
    const options = pythonPreparationCode(snapshot);
    const built = modulesFor(snapshot, false, undefined);
    const versionId = envelope.loaderKey.split("/")[2]!;
    const code = {
      ...options,
      mainModule: built.mainModule,
      modules: built.modules,
      ...tenantEnv(
        snapshot,
        built.policy,
        ctx,
        env.WORKER_LOADER_FACTORY,
        versionId,
        doPolicy(env),
      ),
      globalOutbound: tenantGlobalOutbound(env, false),
    };
    const namespace = `prepare/${envelope.loaderKey}/${envelope.expected}/${systemRequestId()}`;
    try {
      const loader = env.WORKER_LOADER_FACTORY.getPrivate(namespace);
      const bytes = await loader.preparePython(code);
      if (bytes.byteLength < 16 || bytes.byteLength > 128 * 1024 * 1024)
        throw bindingError("VERSION_INVARIANT_VIOLATION");
      return new Response(bytes, {
        headers: {
          "content-type": "application/octet-stream",
          "content-length": String(bytes.byteLength),
          "cache-control": "no-store",
        },
      });
    } finally {
      env.WORKER_LOADER_FACTORY.revoke(namespace);
    }
  } catch (error) {
    const candidate = stableCode(error);
    const code =
      candidate === "RUNTIME_UNAVAILABLE" ||
      candidate === "ARTIFACT_UNAVAILABLE" ||
      candidate === "VERSION_NOT_READY"
        ? candidate
        : "BUNDLE_RUNTIME_INVALID";
    // Native import errors and tracebacks can contain tenant source or secret values.
    return new Response(null, {
      status:
        code === "VERSION_NOT_READY"
          ? 409
          : code === "BUNDLE_RUNTIME_INVALID"
            ? 422
            : 503,
      headers: {
        "x-open-compute-error-code": code,
      },
    });
  }
}

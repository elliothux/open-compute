import { bindingBody } from "../bindings/json-body.js";
import type {
  LoaderEnv,
  RuntimeSnapshot,
  RuntimeSourceScope,
} from "./protocol.js";
import { bindingError } from "./shared.js";

const MAX_SNAPSHOT_BYTES = 128 * 1024 * 1024;

/** Fetch only the retained, verified binary artifact for this source identity. */
export async function resolvePythonSnapshot(
  env: LoaderEnv,
  snapshot: RuntimeSnapshot,
  scope: RuntimeSourceScope,
): Promise<Uint8Array<ArrayBuffer>> {
  const response = await env.RUNTIME_SOURCE.fetch(
    "http://runtime-source/internal/runtime/v1/versions/python-snapshot",
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify({
        key: snapshot.loaderKey,
        expectedWorkerCodeSha256: snapshot.workerCodeSha256,
        expectedPreparedSha256: snapshot.pythonPreparedSha256,
        scope,
      }),
    },
  ).catch(() => {
    throw bindingError("RUNTIME_UNAVAILABLE");
  });
  const length = response.headers.get("content-length");
  if (
    !response.ok ||
    response.headers.get("content-type") !== "application/octet-stream" ||
    response.headers.has("content-encoding") ||
    (length !== null &&
      (!/^[0-9]+$/.test(length) || Number(length) > MAX_SNAPSHOT_BYTES))
  ) {
    await response.body?.cancel().catch(() => {});
    const code = response.headers.get("x-open-compute-error-code");
    throw bindingError(
      code && /^[A-Z][A-Z0-9_]{0,63}$/.test(code)
        ? code
        : "VERSION_INVARIANT_VIOLATION",
    );
  }
  const body = bindingBody(
    response.body,
    MAX_SNAPSHOT_BYTES,
    "VERSION_INVARIANT_VIOLATION",
  );
  const bytes = new Uint8Array(
    await new Response(body).arrayBuffer().catch(() => {
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    }),
  );
  if (
    bytes.byteLength < 16 ||
    (length !== null && Number(length) !== bytes.byteLength)
  )
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  return bytes;
}

import type { NativeWorkerLoaderFactory } from "./protocol.js";

function namespaceKeys(value: unknown): value is string[] {
  return Array.isArray(value) && value.length > 0 && value.length <= 128
    && value.every((key: unknown) => typeof key === "string" && /^[0-9a-f]{64}$/.test(key));
}

/** Revoke a bounded, generation-authenticated batch from committed Script deletion. */
export async function revokeWorkerLoaders(
  request: Request, factory: NativeWorkerLoaderFactory,
): Promise<Response> {
  const length = Number(request.headers.get("content-length"));
  if (request.headers.get("content-type") !== "application/json"
      || !Number.isSafeInteger(length) || length < 2 || length > 16_384) {
    return new Response(null, { status: 400 });
  }
  let keys: unknown;
  try { keys = await request.json(); } catch { return new Response(null, { status: 400 }); }
  if (!namespaceKeys(keys)) {
    return new Response(null, { status: 400 });
  }
  for (const key of keys) factory.revoke(key);
  return new Response(null, { status: 204 });
}

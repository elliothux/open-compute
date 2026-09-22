import type { NativeWorkerLoaderFactory } from "./protocol.js";

function namespacePrefixes(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.length <= 128 &&
    value.every(
      (prefix: unknown) =>
        typeof prefix === "string" &&
        /^[0-9a-f]{64}\/(?:[0-9a-f]{16}\/)?$/.test(prefix),
    )
  );
}

/** Revoke a bounded, generation-authenticated batch of Script or route epochs. */
export async function revokeWorkerLoaders(
  request: Request,
  factory: NativeWorkerLoaderFactory,
): Promise<Response> {
  const length = Number(request.headers.get("content-length"));
  if (
    request.headers.get("content-type") !== "application/json" ||
    !Number.isSafeInteger(length) ||
    length < 2 ||
    length > 16_384
  ) {
    return new Response(null, { status: 400 });
  }
  let prefixes: unknown;
  try {
    prefixes = await request.json();
  } catch {
    return new Response(null, { status: 400 });
  }
  if (!namespacePrefixes(prefixes)) {
    return new Response(null, { status: 400 });
  }
  for (const prefix of prefixes) factory.revokePrefix(prefix);
  return new Response(null, { status: 204 });
}

import type { Env } from "../env";
import { jsonError, ok } from "../lib/json";

export async function handleKv(
  request: Request,
  env: Env,
  key: string,
): Promise<Response> {
  if (request.method === "PUT") {
    const body = await request.text();
    const metadataRaw = request.headers.get("x-kv-metadata");
    const metadata = metadataRaw ? { note: metadataRaw } : undefined;
    await env.KV.put(key, body, metadata ? { metadata } : undefined);
    const listed = await env.KV.list({
      prefix: key.slice(0, Math.min(8, key.length)),
    });
    return ok({
      stack: "kv",
      key,
      written: body.length,
      metadata,
      listSample: listed.keys.slice(0, 5).map((entry) => entry.name),
    });
  }

  if (request.method === "GET") {
    const value = await env.KV.get(key);
    const withMetadata = await env.KV.getWithMetadata<{ note?: string }>(key);
    return ok({
      stack: "kv",
      key,
      value,
      metadata: withMetadata.metadata ?? null,
      exists: value !== null,
    });
  }

  return jsonError("method_not_allowed", 405);
}

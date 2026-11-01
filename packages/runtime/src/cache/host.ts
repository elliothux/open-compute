import { WorkerEntrypoint } from "cloudflare:workers";
import {
  bindingJson,
  expectBindingStatus,
  framed,
  isRecord,
} from "../bindings/private-transport.js";
import type { BindingEnv, CacheTransportProps } from "../bindings/protocol.js";
import { bindingError, INTERNAL_HEADERS } from "../loader/shared.js";
import { nativeCacheRequest, nativeCacheResponse } from "./native-adapter.js";

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);
const WRITE_CONTEXT_HEADER = "x-open-compute-cache-write-context";
type CacheNamespace = "automatic" | "default" | "named";
type CacheFence = { fenceGeneration: string; refreshToken?: string };

function cacheProps(value: unknown): CacheTransportProps {
  if (
    !isRecord(value) ||
    Object.keys(value).length !== 7 ||
    typeof value.instanceId !== "string" ||
    typeof value.workerId !== "string" ||
    typeof value.versionId !== "string" ||
    typeof value.entrypoint !== "string" ||
    typeof value.descriptorSha256 !== "string" ||
    !/^[0-9a-f]{64}$/.test(value.descriptorSha256) ||
    typeof value.automaticEnabled !== "boolean" ||
    typeof value.crossVersionCache !== "boolean"
  )
    throw bindingError("CACHE_PROTOCOL_ERROR");
  return {
    instanceId: value.instanceId,
    workerId: value.workerId,
    versionId: value.versionId,
    entrypoint: value.entrypoint,
    descriptorSha256: value.descriptorSha256,
    automaticEnabled: value.automaticEnabled,
    crossVersionCache: value.crossVersionCache,
  };
}

function cacheError(error: unknown): Error {
  const code: unknown =
    error instanceof Error
      ? Object.getOwnPropertyDescriptor(error, "stableCode")?.value
      : undefined;
  return bindingError(
    typeof code === "string" &&
      /^(?:CACHE|BINDING)_[A-Z0-9_]{1,127}$/.test(code)
      ? code
      : "CACHE_PROTOCOL_ERROR",
  );
}

function publicHeaders(input: Headers): Array<[string, string]> {
  const headers: Array<[string, string]> = [];
  const connectionFields = new Set(
    (input.get("connection") ?? "")
      .split(",")
      .map((name) => name.trim().toLowerCase()),
  );
  // Native Cache HTTP framing is transport metadata, never part of the persisted representation.
  for (const [name, value] of input) {
    if (
      !name.startsWith("x-open-compute-") &&
      !INTERNAL_HEADERS.includes(name) &&
      !HOP_BY_HOP_HEADERS.has(name) &&
      !connectionFields.has(name)
    ) {
      headers.push([name.toLowerCase(), value]);
    }
  }
  return headers;
}

/** Private per-entrypoint Cache transport; never exposed directly to tenant code. */
export class CacheTransport extends WorkerEntrypoint<
  BindingEnv,
  CacheTransportProps
> {
  #props() {
    return cacheProps(this.ctx.props);
  }

  #headers() {
    const props = this.#props();
    return {
      "x-open-compute-instance-id": props.instanceId,
      "x-open-compute-worker-id": props.workerId,
      "x-open-compute-version-id": props.versionId,
      "x-open-compute-entrypoint": props.entrypoint,
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-cache-automatic-enabled": String(props.automaticEnabled),
      "x-open-compute-cache-cross-version": String(props.crossVersionCache),
      "x-open-compute-request-id": crypto.randomUUID(),
    };
  }

  async #fetch(
    path: string,
    init: RequestInit,
    cacheMatch = false,
  ): Promise<Response> {
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend${path}`,
      {
        ...init,
        ...(cacheMatch ? { encodeResponseBody: "manual" as const } : {}),
        headers: { ...this.#headers(), ...init.headers },
      },
    );
    const code = response.headers.get("x-open-compute-error-code");
    if (code || (!cacheMatch && !response.ok)) {
      try {
        await response.body?.cancel();
      } catch {
        /* best effort */
      }
      throw bindingError(code || "CACHE_PROTOCOL_ERROR");
    }
    return response;
  }

  async match(
    namespace: "automatic" | "default" | "named",
    name: string | undefined,
    request: Request,
  ): Promise<{
    status:
      "HIT" | "MISS" | "EXPIRED" | "UPDATING" | "STALE" | "STALE_IF_ERROR";
    fenceGeneration: string;
    refreshToken?: string;
    response?: Response;
  }> {
    const response = await this.#fetch(
      "/internal/cache/v1/match",
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          namespace,
          name,
          url: request.url,
          method: request.method,
          headers: publicHeaders(request.headers),
        }),
      },
      true,
    );
    const status = response.headers.get("x-open-compute-cache-status");
    const fenceGeneration = response.headers.get("x-open-compute-cache-fence");
    const refreshToken =
      response.headers.get("x-open-compute-cache-refresh-token") ?? undefined;
    if (
      !status ||
      ![
        "HIT",
        "MISS",
        "EXPIRED",
        "UPDATING",
        "STALE",
        "STALE_IF_ERROR",
      ].includes(status) ||
      !fenceGeneration ||
      !/^[1-9][0-9]{0,19}$/.test(fenceGeneration) ||
      (refreshToken !== undefined && !/^[0-9a-f]{32}$/.test(refreshToken)) ||
      (status === "UPDATING") !== (refreshToken !== undefined)
    ) {
      try {
        await response.body?.cancel();
      } catch {
        /* best effort */
      }
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    const lookup = {
      status: status as
        "HIT" | "MISS" | "EXPIRED" | "UPDATING" | "STALE" | "STALE_IF_ERROR",
      fenceGeneration,
      ...(refreshToken === undefined ? {} : { refreshToken }),
    };
    const hit = response.headers.get("x-open-compute-cache-hit") === "1";
    const responseStatus = [
      "HIT",
      "UPDATING",
      "STALE",
      "STALE_IF_ERROR",
    ].includes(status);
    if (hit !== responseStatus || (!hit && response.status !== 204)) {
      try {
        await response.body?.cancel();
      } catch {
        /* best effort */
      }
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    if (!hit) {
      try {
        await response.body?.cancel();
      } catch {
        /* best effort */
      }
      return lookup;
    }
    const headers = new Headers(response.headers);
    for (const key of Array.from(headers.keys()))
      if (key.startsWith("x-open-compute-")) headers.delete(key);
    return {
      ...lookup,
      response: new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
        encodeBody: "manual",
      }),
    };
  }

  async putAutomatic(
    request: Request,
    response: Response,
    fence: CacheFence,
  ): Promise<void> {
    // Native Response::send owns compression and body consumption for both cache surfaces.
    const headers = new Headers(request.headers);
    const context = JSON.stringify({
      props: this.#props(),
      fence: {
        fenceGeneration: fence.fenceGeneration,
        refreshToken: fence.refreshToken,
      },
    });
    if (new TextEncoder().encode(context).byteLength > 64 * 1024)
      throw bindingError("CACHE_LIMIT_EXCEEDED");
    headers.set(WRITE_CONTEXT_HEADER, context);
    await caches.default.put(new Request(request.url, { headers }), response);
  }

  /** Store the opaque bytes already serialized by workerd's Cache client. */
  async storeEncoded(
    namespace: CacheNamespace,
    name: string | undefined,
    request: Request,
    response: Response,
    fence?: CacheFence,
  ): Promise<void> {
    const metadata = {
      namespace,
      name,
      url: request.url,
      method: request.method,
      headers: publicHeaders(request.headers),
      status: response.status,
      responseHeaders: publicHeaders(response.headers),
      ...(fence === undefined
        ? {}
        : {
            expectedFenceGeneration: fence.fenceGeneration,
            ...(fence.refreshToken === undefined
              ? {}
              : { refreshToken: fence.refreshToken }),
          }),
    };
    const result = await this.#fetch("/internal/cache/v1/put", {
      method: "POST",
      headers: {
        "content-type": "application/vnd.open-compute.cache.v1+frame",
      },
      body: framed(metadata, response.body, "CACHE_LIMIT_EXCEEDED"),
    });
    await expectBindingStatus(result, 204, "CACHE_PROTOCOL_ERROR");
    try {
      await result.body?.cancel();
    } catch {
      /* best effort */
    }
  }

  /** Native Cache backend; scoped through the same persisted authority as automatic caching. */
  async fetch(request: Request): Promise<Response> {
    try {
      return await nativeCacheRequest(this, request);
    } catch (error) {
      try {
        await request.body?.cancel();
      } catch {
        /* best effort */
      }
      throw cacheError(error);
    }
  }

  async delete(
    namespace: "default" | "named",
    name: string | undefined,
    request: Request,
  ): Promise<boolean> {
    const response = await this.#fetch("/internal/cache/v1/delete", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        namespace,
        name,
        url: request.url,
        method: request.method,
        headers: publicHeaders(request.headers),
      }),
    });
    await expectBindingStatus(response, 200, "CACHE_PROTOCOL_ERROR");
    const value = await bindingJson(response, "CACHE_PROTOCOL_ERROR");
    if (
      !isRecord(value) ||
      Object.keys(value).length !== 1 ||
      typeof value.deleted !== "boolean"
    ) {
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    return value.deleted;
  }

  async purge(
    options: unknown,
  ): Promise<{ success: boolean; deleted: number }> {
    const response = await this.#fetch("/internal/cache/v1/purge", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(options),
    });
    await expectBindingStatus(response, 200, "CACHE_PROTOCOL_ERROR");
    const value = await bindingJson(response, "CACHE_PROTOCOL_ERROR");
    if (
      !isRecord(value) ||
      Object.keys(value).length !== 2 ||
      value.success !== true ||
      !Number.isSafeInteger(value.deleted) ||
      (value.deleted as number) < 0
    ) {
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    return { success: true, deleted: value.deleted as number };
  }
}

/** Host-only native serializer sink; never delegated through tenant env or Loader fields. */
export class CacheWriteTransport extends WorkerEntrypoint<BindingEnv> {
  async fetch(request: Request): Promise<Response> {
    try {
      if (request.method !== "PUT") throw bindingError("CACHE_PROTOCOL_ERROR");
      const raw = request.headers.get(WRITE_CONTEXT_HEADER);
      if (raw === null || new TextEncoder().encode(raw).byteLength > 64 * 1024)
        throw bindingError("CACHE_PROTOCOL_ERROR");
      const value: unknown = JSON.parse(raw);
      if (
        !isRecord(value) ||
        Object.keys(value).length !== 2 ||
        Object.keys(value).some((key) => !["props", "fence"].includes(key))
      )
        throw bindingError("CACHE_PROTOCOL_ERROR");
      const props = cacheProps(value.props);
      const fence = value.fence;
      if (
        !isRecord(fence) ||
        Object.keys(fence).some(
          (key) => !["fenceGeneration", "refreshToken"].includes(key),
        ) ||
        typeof fence.fenceGeneration !== "string" ||
        !/^[1-9][0-9]{0,19}$/.test(fence.fenceGeneration) ||
        (fence.refreshToken !== undefined &&
          (typeof fence.refreshToken !== "string" ||
            !/^[0-9a-f]{32}$/.test(fence.refreshToken)))
      )
        throw bindingError("CACHE_PROTOCOL_ERROR");
      const response = await nativeCacheResponse(request);
      const headers = new Headers(request.headers);
      headers.delete(WRITE_CONTEXT_HEADER);
      try {
        await this.ctx.exports
          .CacheTransport({ props })
          .storeEncoded(
            "automatic",
            undefined,
            new Request(request.url, { headers }),
            response,
            {
              fenceGeneration: fence.fenceGeneration,
              ...(fence.refreshToken === undefined
                ? {}
                : { refreshToken: fence.refreshToken }),
            },
          );
      } catch (error) {
        try {
          await response.body?.cancel();
        } catch {
          /* best effort */
        }
        throw error;
      }
      return new Response(null, { status: 204 });
    } catch (error) {
      try {
        await request.body?.cancel();
      } catch {
        /* best effort */
      }
      throw cacheError(error);
    }
  }
}

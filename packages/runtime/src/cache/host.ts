import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv, CacheTransportProps } from "../bindings/protocol.js";
import {
  bindingJson, expectBindingStatus, framed, isRecord,
} from "../bindings/private-transport.js";
import {
  bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration, INTERNAL_HEADERS,
} from "../loader/shared.js";

function publicHeaders(input: Headers): Array<[string, string]> {
  const headers: Array<[string, string]> = [];
  for (const [name, value] of input) {
    if (!name.startsWith("x-open-compute-") && !INTERNAL_HEADERS.includes(name)) {
      headers.push([name.toLowerCase(), value]);
    }
  }
  return headers;
}

/** Private per-entrypoint Cache transport; never exposed directly to tenant code. */
export class CacheTransport extends WorkerEntrypoint<BindingEnv, CacheTransportProps> {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.accountId !== "string" || typeof props.workerId !== "string"
        || typeof props.deploymentId !== "string" || typeof props.entrypoint !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || typeof props.automaticEnabled !== "boolean" || typeof props.crossVersionCache !== "boolean") {
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    return props;
  }

  #headers() {
    const props = this.#props();
    return {
      [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-account-id": props.accountId,
      "x-open-compute-worker-id": props.workerId,
      "x-open-compute-deployment-id": props.deploymentId,
      "x-open-compute-entrypoint": props.entrypoint,
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-cache-automatic-enabled": String(props.automaticEnabled),
      "x-open-compute-cache-cross-version": String(props.crossVersionCache),
      "x-open-compute-request-id": crypto.randomUUID(),
    };
  }

  async #fetch(path: string, init: RequestInit, cacheMatch = false): Promise<Response> {
    const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend${path}`, {
      ...init,
      headers: { ...this.#headers(), ...(init.headers ?? {}) },
    });
    const code = response.headers.get("x-open-compute-error-code");
    if (code || (!cacheMatch && !response.ok)) {
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError(code || "CACHE_PROTOCOL_ERROR");
    }
    return response;
  }

  async match(namespace: "automatic" | "default" | "named", name: string | undefined,
    request: Request): Promise<{
      status: "HIT" | "MISS" | "EXPIRED" | "UPDATING" | "STALE" | "STALE_IF_ERROR";
      fenceGeneration: string;
      refreshToken?: string;
      response?: Response;
    }> {
    const response = await this.#fetch("/internal/cache/v1/match", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ namespace, name, url: request.url, method: request.method,
        headers: publicHeaders(request.headers) }),
    }, true);
    const status = response.headers.get("x-open-compute-cache-status");
    const fenceGeneration = response.headers.get("x-open-compute-cache-fence");
    const refreshToken = response.headers.get("x-open-compute-cache-refresh-token") ?? undefined;
    if (!status || !["HIT", "MISS", "EXPIRED", "UPDATING", "STALE", "STALE_IF_ERROR"].includes(status)
        || !fenceGeneration || !/^[1-9][0-9]{0,19}$/.test(fenceGeneration)
        || (refreshToken !== undefined && !/^[0-9a-f]{32}$/.test(refreshToken))
        || (status === "UPDATING") !== (refreshToken !== undefined)) {
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    const lookup = {
      status: status as "HIT" | "MISS" | "EXPIRED" | "UPDATING" | "STALE" | "STALE_IF_ERROR",
      fenceGeneration,
      ...(refreshToken === undefined ? {} : { refreshToken }),
    };
    const hit = response.headers.get("x-open-compute-cache-hit") === "1";
    const responseStatus = ["HIT", "UPDATING", "STALE", "STALE_IF_ERROR"].includes(status);
    if (hit !== responseStatus || (!hit && response.status !== 204)) {
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    if (!hit) {
      try { await response.body?.cancel(); } catch { /* best effort */ }
      return lookup;
    }
    const headers = new Headers(response.headers);
    for (const key of [...headers.keys()]) if (key.startsWith("x-open-compute-")) headers.delete(key);
    return { ...lookup,
      response: new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      }),
    };
  }

  async put(namespace: "automatic" | "default" | "named", name: string | undefined,
    request: Request, response: Response,
    fence?: { fenceGeneration: string; refreshToken?: string }): Promise<void> {
    const metadata = { namespace, name, url: request.url, method: request.method,
      headers: publicHeaders(request.headers), status: response.status,
      responseHeaders: publicHeaders(response.headers),
      ...(fence === undefined ? {} : {
        expectedFenceGeneration: fence.fenceGeneration,
        ...(fence.refreshToken === undefined ? {} : { refreshToken: fence.refreshToken }),
      }),
    };
    const result = await this.#fetch("/internal/cache/v1/put", {
      method: "POST", headers: { "content-type": "application/vnd.open-compute.cache.v1+frame" },
      body: framed(metadata, response.body, "CACHE_LIMIT_EXCEEDED"),
    });
    await expectBindingStatus(result, 204, "CACHE_PROTOCOL_ERROR");
    try { await result.body?.cancel(); } catch { /* best effort */ }
  }

  async delete(namespace: "default" | "named", name: string | undefined,
    request: Request): Promise<boolean> {
    const response = await this.#fetch("/internal/cache/v1/delete", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ namespace, name, url: request.url, method: request.method,
        headers: publicHeaders(request.headers) }),
    });
    await expectBindingStatus(response, 200, "CACHE_PROTOCOL_ERROR");
    const value = await bindingJson(response, "CACHE_PROTOCOL_ERROR");
    if (!isRecord(value) || Object.keys(value).length !== 1 || typeof value.deleted !== "boolean") {
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    return value.deleted;
  }

  async purge(options: unknown): Promise<{ success: boolean; deleted: number }> {
    const response = await this.#fetch("/internal/cache/v1/purge", {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(options),
    });
    await expectBindingStatus(response, 200, "CACHE_PROTOCOL_ERROR");
    const value = await bindingJson(response, "CACHE_PROTOCOL_ERROR");
    if (!isRecord(value) || Object.keys(value).length !== 2 || value.success !== true
        || !Number.isSafeInteger(value.deleted) || (value.deleted as number) < 0) {
      throw bindingError("CACHE_PROTOCOL_ERROR");
    }
    return { success: true, deleted: value.deleted as number };
  }
}

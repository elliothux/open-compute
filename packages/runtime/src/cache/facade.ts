const PRIVATE_CACHE = "__OPEN_COMPUTE_PRIVATE_CACHE";
const CACHE_NAME = /^[^\u0000-\u001f\u007f]{1,128}$/;
const CACHEABLE_STATUS = new Set([200, 203, 204, 300, 301, 404, 405, 410, 414, 501]);

interface MatchOptions { ignoreMethod?: boolean }
interface PurgeOptions { tags?: string[]; pathPrefixes?: string[]; purgeEverything?: boolean }
type CacheLookupStatus = "HIT" | "MISS" | "EXPIRED" | "UPDATING" | "STALE" | "STALE_IF_ERROR";
interface CacheWriteFence {
  fenceGeneration: string;
  refreshToken?: string;
}
interface CacheLookup extends CacheWriteFence {
  status: CacheLookupStatus;
  response?: Response;
}
interface CacheTransport {
  match(namespace: "automatic" | "default" | "named", name: string | undefined,
    request: Request): Promise<CacheLookup>;
  put(namespace: "automatic" | "default" | "named", name: string | undefined,
    request: Request, response: Response, fence?: CacheWriteFence): Promise<void>;
  delete(namespace: "default" | "named", name: string | undefined, request: Request): Promise<boolean>;
  purge(options: PurgeOptions): Promise<{ success: boolean; deleted: number }>;
}

const LOOKUP_STATUSES = new Set<CacheLookupStatus>([
  "HIT", "MISS", "EXPIRED", "UPDATING", "STALE", "STALE_IF_ERROR",
]);
const RESPONSE_LOOKUP_STATUSES = new Set<CacheLookupStatus>([
  "HIT", "UPDATING", "STALE", "STALE_IF_ERROR",
]);

let activeTransport: CacheTransport | undefined;

function transport(): CacheTransport {
  if (activeTransport === undefined) throw new Error("CACHE_UNAVAILABLE");
  return activeTransport;
}

function bindTransport(environment: object, entrypoint: string): CacheTransport {
  const transports: unknown = Reflect.get(environment, PRIVATE_CACHE);
  const value: unknown = transports !== null && typeof transports === "object"
    ? Reflect.get(transports, entrypoint)
    : undefined;
  if (value === null || typeof value !== "object"
      || typeof Reflect.get(value, "match") !== "function"
      || typeof Reflect.get(value, "put") !== "function"
      || typeof Reflect.get(value, "delete") !== "function"
      || typeof Reflect.get(value, "purge") !== "function") {
    throw new Error("CACHE_UNAVAILABLE");
  }
  const bound = value as CacheTransport;
  activeTransport = bound;
  return bound;
}

function requestOf(value: RequestInfo): Request {
  const request = value instanceof Request ? value : new Request(value);
  const url = new URL(request.url);
  if (!['http:', 'https:'].includes(url.protocol) || url.hash) throw new TypeError("CACHE_KEY_INVALID");
  return request;
}

function validateOptions(options: MatchOptions | undefined): void {
  if (options === undefined) return;
  if (options === null || typeof options !== "object" || Array.isArray(options)
      || Object.keys(options).some(key => key !== "ignoreMethod")
      || (options.ignoreMethod !== undefined && typeof options.ignoreMethod !== "boolean")) {
    throw new TypeError("CACHE_PROTOCOL_ERROR");
  }
}

function cacheLookup(value: unknown): CacheLookup {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError("CACHE_PROTOCOL_ERROR");
  }
  const status: unknown = Reflect.get(value, "status");
  const fenceGeneration: unknown = Reflect.get(value, "fenceGeneration");
  const refreshToken: unknown = Reflect.get(value, "refreshToken");
  const response: unknown = Reflect.get(value, "response");
  if (typeof status !== "string" || !LOOKUP_STATUSES.has(status as CacheLookupStatus)
      || typeof fenceGeneration !== "string" || !/^[1-9][0-9]{0,19}$/.test(fenceGeneration)
      || (refreshToken !== undefined && (typeof refreshToken !== "string"
        || !/^[0-9a-f]{32}$/.test(refreshToken)))
      || (status === "UPDATING") !== (refreshToken !== undefined)
      || (response !== undefined && !(response instanceof Response))
      || RESPONSE_LOOKUP_STATUSES.has(status as CacheLookupStatus) !== (response !== undefined)) {
    throw new TypeError("CACHE_PROTOCOL_ERROR");
  }
  return value as CacheLookup;
}

class LocalCache {
  readonly #namespace: "default" | "named";
  readonly #name: string | undefined;

  constructor(namespace: "default" | "named", name?: string) {
    this.#namespace = namespace;
    this.#name = name;
  }

  async match(value: RequestInfo, options?: MatchOptions): Promise<Response | undefined> {
    validateOptions(options);
    const request = requestOf(value);
    if (request.method !== "GET" && request.method !== "HEAD") {
      if (options?.ignoreMethod === true) {
        return cacheLookup(await transport().match(
          this.#namespace,
          this.#name,
          new Request(request, { method: "GET" }),
        )).response;
      }
      return undefined;
    }
    return cacheLookup(await transport().match(this.#namespace, this.#name, request)).response;
  }

  async put(value: RequestInfo, response: Response): Promise<void> {
    const request = requestOf(value);
    if (!(response instanceof Response)) throw new TypeError("CACHE_PROTOCOL_ERROR");
    if (request.method !== "GET" || response.status === 206
        || response.headers.get("vary")?.split(",").some(value => value.trim() === "*")) {
      throw new TypeError("CACHE_PUT_REJECTED");
    }
    await transport().put(this.#namespace, this.#name, request, response);
  }

  async delete(value: RequestInfo, options?: MatchOptions): Promise<boolean> {
    validateOptions(options);
    const request = requestOf(value);
    if (request.method !== "GET" && options?.ignoreMethod !== true) return false;
    const deleted: unknown = await transport().delete(this.#namespace, this.#name,
      request.method === "GET" ? request : new Request(request, { method: "GET" }));
    if (typeof deleted !== "boolean") throw new TypeError("CACHE_PROTOCOL_ERROR");
    return deleted;
  }
}

class LocalCacheStorage {
  readonly default = (() => {
    const cache = new LocalCache("default");
    Object.freeze(cache);
    return cache;
  })();

  async open(name: string): Promise<LocalCache> {
    if (typeof name !== "string" || !CACHE_NAME.test(name)) throw new TypeError("CACHE_KEY_INVALID");
    const cache = new LocalCache("named", name);
    Object.freeze(cache);
    return cache;
  }
}

const storage = Object.freeze(new LocalCacheStorage());
Object.defineProperty(globalThis, "caches", {
  value: storage,
  configurable: false,
  enumerable: true,
  writable: false,
});

export interface CacheRuntime {
  readonly context: { purge(options: PurgeOptions): Promise<{ success: boolean; deleted: number }> };
  dispatch(origin: () => unknown, request: Request, ctx: ExecutionContext): Promise<Response>;
}

export interface CacheRuntimeFactory {
  bind(environment: object): CacheRuntime | undefined;
}

function cacheable(request: Request, response: Response): boolean {
  const requestControl = request.headers.get("cache-control")?.toLowerCase() ?? "";
  const responseControl = (response.headers.get("cloudflare-cdn-cache-control")
    ?? response.headers.get("cdn-cache-control")
    ?? response.headers.get("cache-control")
    ?? "").toLowerCase();
  return request.method === "GET" && !request.headers.has("authorization")
    && !response.headers.has("set-cookie") && CACHEABLE_STATUS.has(response.status)
    && !hasDirective(`${requestControl},${responseControl}`, new Set(["no-store", "no-cache", "private"]))
    && hasExplicitTtl(responseControl);
}

function hasDirective(value: string, names: ReadonlySet<string>): boolean {
  return value.split(",").some(part => {
    const equals = part.indexOf("=");
    const name = (equals === -1 ? part : part.slice(0, equals)).trim();
    return names.has(name);
  });
}

function hasExplicitTtl(value: string): boolean {
  return value.split(",").some(part => {
    const equals = part.indexOf("=");
    if (equals === -1) return false;
    const name = part.slice(0, equals).trim();
    const seconds = part.slice(equals + 1).trim();
    return ["s-maxage", "max-age"].includes(name) && /^(?:[0-9]+|"[0-9]+")$/.test(seconds);
  });
}

function withCacheStatus(response: Response, status: string): Response {
  const headers = new Headers(response.headers);
  headers.delete("cache-tag");
  headers.set("cf-cache-status", status);
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

async function originResponse(origin: () => unknown): Promise<Response> {
  const response = await origin();
  if (!(response instanceof Response)) throw new TypeError("CACHE_PROTOCOL_ERROR");
  return response;
}

async function discardResponse(response: Response): Promise<void> {
  try {
    await response.body?.cancel();
  } catch {
    // The response is already hidden from the tenant; cancellation is best effort.
  }
}

function cacheFailureCode(error: unknown): string {
  if (error === null || typeof error !== "object") return "CACHE_PROTOCOL_ERROR";
  for (const key of ["stableCode", "message"]) {
    let descriptor: PropertyDescriptor | undefined;
    try {
      descriptor = Object.getOwnPropertyDescriptor(error, key);
    } catch {
      return "CACHE_PROTOCOL_ERROR";
    }
    if (typeof descriptor?.value === "string" && [
      "CACHE_UNAVAILABLE",
      "CACHE_RESULT_UNKNOWN",
      "CACHE_CORRUPT",
      "CACHE_PROTOCOL_ERROR",
    ].includes(descriptor.value)) return descriptor.value;
  }
  return "CACHE_PROTOCOL_ERROR";
}

/** Build the automatic dispatcher only for an explicitly enabled fetch entrypoint. */
export function createCacheRuntime(
  enabled: boolean,
  failOpen: boolean,
  entrypoint = "default",
): CacheRuntimeFactory {
  return Object.freeze({
    bind(environment: object): CacheRuntime | undefined {
      const raw = bindTransport(environment, entrypoint);
      if (!enabled) return undefined;
      return Object.freeze({
        context: Object.freeze({ purge: async (options: PurgeOptions) => {
          const value: unknown = await raw.purge(options);
          if (value === null || typeof value !== "object" || Array.isArray(value)
              || Reflect.get(value, "success") !== true
              || !Number.isSafeInteger(Reflect.get(value, "deleted"))
              || (Reflect.get(value, "deleted") as number) < 0) {
            throw new TypeError("CACHE_PROTOCOL_ERROR");
          }
          return value as { success: true; deleted: number };
        } }),
        async dispatch(origin: () => unknown, request: Request, ctx: ExecutionContext): Promise<Response> {
          if (!(request instanceof Request) || !["GET", "HEAD"].includes(request.method)) {
            return withCacheStatus(await originResponse(origin), "BYPASS");
          }
          let lookup: CacheLookup;
          try {
            lookup = cacheLookup(await raw.match("automatic", undefined, request));
          } catch (error) {
            const code = cacheFailureCode(error);
            if (!failOpen || !["CACHE_UNAVAILABLE", "CACHE_RESULT_UNKNOWN"].includes(code)) {
              throw new Error(code);
            }
            return withCacheStatus(await originResponse(origin), "BYPASS");
          }
          if (lookup.response !== undefined) {
            if (lookup.status === "HIT" || lookup.status === "STALE") return lookup.response;
            if (lookup.status === "UPDATING") {
              const refresh = originResponse(origin).then(async response => {
                if (!cacheable(request, response)) {
                  await discardResponse(response);
                  return;
                }
                return raw.put("automatic", undefined, request, response, lookup);
              }).catch(() => undefined);
              ctx.waitUntil(refresh);
              return lookup.response;
            }
            if (lookup.status === "STALE_IF_ERROR") {
              try {
                const response = await originResponse(origin);
                if (response.status >= 500) {
                  await discardResponse(response);
                  return withCacheStatus(lookup.response, "STALE");
                }
                if (cacheable(request, response)) {
                  ctx.waitUntil(raw.put(
                    "automatic", undefined, request, response.clone(), lookup,
                  ).catch(() => undefined));
                }
                return withCacheStatus(response, cacheable(request, response) ? "REVALIDATED" : "BYPASS");
              } catch {
                return withCacheStatus(lookup.response, "STALE");
              }
            }
          }
          const response = await originResponse(origin);
          if (!cacheable(request, response) || request.method !== "GET") {
            return withCacheStatus(response, "BYPASS");
          }
          const store = raw.put(
            "automatic", undefined, request, response.clone(), lookup,
          ).catch(() => undefined);
          ctx.waitUntil(store);
          return withCacheStatus(response, lookup.status === "EXPIRED" ? "EXPIRED" : "MISS");
        },
      });
    },
  });
}

# Cache API

The `cache_api` product: `caches.default`, `caches.open()`, `put` / `match` / `delete`. `caches.default` shares logical storage with that Worker's default HTTP response cache.

```ts
export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const cached = await caches.default.match(request);
    if (cached) return cached;
    const response = new Response("hello", {
      headers: { "Cache-Control": "public, s-maxage=60" },
    });
    ctx.waitUntil(caches.default.put(request, response.clone()));
    return response;
  },
} satisfies ExportedHandler<Env>;
```

Deployment-side `cache.enabled`: [Workers Cache](/en/workers/cache/).

## Same as Cloudflare

Symbols match the [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/). Conditional requests, Vary, and Range follow pinned workerd plus the verified local cache authority.

## Intentional delta

See `OC-CACHE-001` / `OC-CACHE-002` (full text on [Workers Cache](/en/workers/cache/)). No global purge propagation, no Cache Tags as a Cloudflare product, no colo-local cache pretending to be a global CDN.

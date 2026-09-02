# Cache API

`caches.default`, `caches.open()`, `put` / `match` / `delete`. `caches.default` shares logical storage with that Worker's default HTTP response cache.

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

Deployment-side `cache.enabled`: [Workers Cache](/workers/cache/).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `caches.default` / `caches.open` / `put` / `match` / `delete` | Yes — [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/) | Yes |
| Conditional requests, Vary, Range | Yes | Pinned workerd plus the local cache authority |
| Cache scope | Global / colo CDN | Single-node |
| Automatic cache TTL | May include heuristic TTL | Requires explicit `s-maxage` or `max-age`; no heuristic TTL |
| Global purge / Cache Tags | Yes | Not provided |


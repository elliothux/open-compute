# Cache API

`cache_api` 产品：`caches.default`、`caches.open()`、`put` / `match` / `delete`。`caches.default` 与该 Worker 的默认 HTTP response cache 共享逻辑存储。

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

部署侧 `cache.enabled` 见 [Workers Cache](/workers/cache/)。

## 与 Cloudflare 相同

符号与 [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/) 相同。条件请求、Vary、Range 按 pinned workerd + 本机 cache authority 的已验证行为。

## 故意不同

见 `OC-CACHE-001` / `OC-CACHE-002`（[Workers Cache](/workers/cache/) 全文）。没有全球 purge 传播、没有 Cache Tags 作为 Cloudflare 产品、没有 colo 局部缓存伪装成全球 CDN。

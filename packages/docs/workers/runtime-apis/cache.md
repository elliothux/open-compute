# Cache API

`caches.default`、`caches.open()`、`put` / `match` / `delete`。`caches.default` 与该 Worker 的默认 HTTP response cache 共享逻辑存储。

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

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `caches.default` / `caches.open` / `put` / `match` / `delete` | 是，见 [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/) | 是 |
| 条件请求、Vary、Range | 是 | 按 pinned workerd 与本节点 cache authority |
| 缓存范围 | 全球 / colo CDN | 单节点 |
| 自动缓存 TTL | 可含启发式 TTL | 需要显式 `s-maxage` 或 `max-age`；无启发式 TTL |
| 全球 purge / Cache Tags | 是 | 不提供 |


# Workers Cache

部署配置驱动的 HTTP 响应缓存，以及租户 Cache API。

```json
{
  "name": "cached",
  "main": "src/index.ts",
  "cache": { "enabled": true, "cross_version_cache": false },
  "exports": {
    "Admin": { "type": "worker", "cache": { "enabled": false } }
  }
}
```

`cache.enabled` 打开默认 HTTP cache。`cross_version_cache` 允许跨部署版本共享；默认隔离。`exports.<name>` 只能覆盖具名 Worker entrypoint 的缓存策略。自动缓存需要显式 `s-maxage` 或 `max-age`。

Cache API（`caches.default` / `caches.open`）见 [Runtime APIs · Cache](/workers/runtime-apis/cache)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `cache.enabled`、entrypoint override、`cross_version_cache` | 是，见 [Workers Cache configuration](https://developers.cloudflare.com/workers/cache/configuration/) | 配置形状对齐 |
| Cache API 符号 | 是，见 [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/) | 是 |
| 缓存在哪 | 全球 / colo | 本机 |
| 自动缓存 TTL | 可含启发式 TTL | 需要显式 `s-maxage` 或 `max-age`；无启发式 TTL |
| 全球复制 / purge 传播 / tiered cache / Cache Rules / Cache Deception Armor / 套餐相关行为 | 是 | 不提供 |
| 对象大小配额 | Cloudflare 产品配额 | 默认每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节；精确值由 `ocd capabilities --json` 给出 |


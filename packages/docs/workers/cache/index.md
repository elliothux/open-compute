# Workers Cache

部署配置驱动的 HTTP 响应缓存，以及租户 Cache API。配置：

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

## 与 Cloudflare 相同

`cache.enabled`、entrypoint override、`cross_version_cache` 的配置形状对齐 [Workers Cache configuration](https://developers.cloudflare.com/workers/cache/configuration/)。Cache API 符号见 [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/)。

## 故意不同：OC-CACHE-001 / OC-CACHE-002

**OC-CACHE-001.** Workers Cache 和 Cache API 是单节点本机权威。自动缓存需要显式 `s-maxage` 或 `max-age`。不支持启发式 TTL、全球复制/purge 传播、tiered cache、Cache Rules、Cache Deception Armor，以及依赖套餐的行为。

**OC-CACHE-002.** 运维配置的默认值是每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值由 `ocd capabilities --json` 给出。

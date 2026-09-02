# Workers Cache

HTTP response cache driven by deployment config, plus the tenant Cache API.

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

`cache.enabled` turns on the default HTTP cache. `cross_version_cache` allows sharing across deployment versions; isolation is the default. `exports.<name>` may only override cache policy for a named Worker entrypoint. Automatic caching requires an explicit `s-maxage` or `max-age`.

The Cache API (`caches.default` / `caches.open`) is documented at [Runtime APIs · Cache](/en/workers/runtime-apis/cache).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `cache.enabled`, entrypoint overrides, `cross_version_cache` | Yes — [Workers Cache configuration](https://developers.cloudflare.com/workers/cache/configuration/) | Matching config shape |
| Cache API symbols | Yes — [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/) | Yes |
| Cache authority | Global / colo | Single-node local authority |
| Automatic cache TTL | May include heuristic TTL | Requires explicit `s-maxage` or `max-age`; no heuristic TTL |
| Global replication / purge propagation / tiered cache / Cache Rules / Cache Deception Armor / plan-dependent behavior | Yes | Not provided |
| Object size quota | Cloudflare product quota | Default 16 MiB per cached object and 1 GiB of logical body bytes per Worker; live values from `ocd capabilities --json` |


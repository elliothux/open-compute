# Workers Cache

HTTP response cache driven by deployment config, plus the tenant Cache API. Configuration:

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

## Same as Cloudflare

`cache.enabled`, entrypoint overrides, and `cross_version_cache` match the shape of [Workers Cache configuration](https://developers.cloudflare.com/workers/cache/configuration/). Cache API symbols: [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/).

## Intentional delta: OC-CACHE-001 / OC-CACHE-002

**OC-CACHE-001.** Workers Cache and Cache API are single-node local authority. Automatic caching requires an explicit `s-maxage` or `max-age`; heuristic TTL, global replication/purge propagation, tiered cache, Cache Rules, Cache Deception Armor, and plan-dependent behavior are unsupported.

**OC-CACHE-002.** The operator-configured default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. The exact active values are emitted by `ocd capabilities --json`.

# Compatibility

open-compute implements the declared Cloudflare Workers programming model. For each product that is provided, the Worker API matches Cloudflare's documentation. The topology is a single node (one `ocd`, one pinned `workerd`).

Live limits and release identity:

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. Without it, `limits` come from the embedded default config; with an absolute config path, `limits` reflect that file.

Worker-side signatures: [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Index on this site: [API reference](/en/platform/reference/api/). Single-node behavior: [Behavior differences](/en/platform/deviations). Numeric ceilings: [Limits](/en/platform/limits).

## Products

| Product | Worker API | Topology difference |
| --- | --- | --- |
| [Workers](/en/workers/) | Module Workers (`fetch` / `scheduled` / `queue`) | Single-node `workerd`; no global edge |
| [KV](/en/kv/) | `KVNamespace` | Local SQLite; no global replication |
| [R2](/en/r2/) | `R2Bucket` | Object bytes on the configured S3; no global placement |
| [D1](/en/d1/) | `D1Database` | Local SQLite primary; no read replicas or region routing |
| [Durable Objects](/en/durable-objects/) | `DurableObject` / `DurableObjectNamespace` | Placed on the single local workerd process |
| [Alarms](/en/durable-objects/alarms) | `state.storage.setAlarm` | Local scheduler |
| [Queues](/en/queues/) | `Queue` and consumer handlers | Local `scheduler.sqlite`; at-least-once, no global FIFO |
| [Cron](/en/workers/configuration/cron-triggers) | `scheduled` handler | UTC, five fields; recovery projects at most the newest misfire |
| [Workflows](/en/workflows/) | Workflows API | Local SQLite; callbacks are at-least-once until commit |
| [Cache API](/en/workers/runtime-apis/cache) | `caches.default` | Single-node cache |
| [Workers Cache](/en/workers/cache/) | Automatic HTTP cache | Single node; requires explicit `s-maxage` / `max-age` |
| [Static Assets](/en/workers/static-assets/) | Assets binding | Immutable S3 content served locally; no global CDN |
| [Service Bindings](/en/workers/runtime-apis/bindings) | `Fetcher` | Same platform; no cross-region discovery |
| [Deployments](/en/workers/versions-and-deployments/) | Versions, promotion, rollback | Local SQLite and one runtime generation |
| [Images](/en/images/) | Images binding | Bounded local raster transforms; not hosted Cloudflare Images |
| [Version Metadata](/en/workers/runtime-apis/bindings) | Deployment `id` / `tag` / `timestamp` | Produced by local deploy authority |
| [WebSocket hibernation](/en/workers/runtime-apis/websockets) | Hibernatable WebSockets | On the local Durable Object process |

D1 covers database / session / prepared statement / result / meta, errors and bind conversions, atomic batches, and opaque bookmarks. General TCP outbound uses the one public Network and stock workerd's `cloudflare:sockets` / Node socket implementation; named Service / DO `Fetcher.connect()` uses an explicit capability tunnel.

## Runtime

The platform freezes the compatibility date. `open-compute.json` cannot set `compatibilityDate` or flags. Use `runtime.effective_compatibility_date`. Do not swap a workerd beside the binary, search `PATH`, or download another runtime.

Products that are not provided: [Unsupported](/en/platform/unsupported). Operators can inspect the running binary with `ocd capabilities --json`.

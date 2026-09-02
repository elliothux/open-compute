# Compatibility

open-compute implements the declared Cloudflare Workers programming model. For each product that is provided, the Worker API matches Cloudflare's documentation. The topology is a single node (one `ocd`, one pinned `workerd`).

Live limits and release identity:

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. Without it, `limits` come from the embedded default config; with an absolute config path, `limits` reflect that file.

Worker-side signatures: [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Index on this site: [API reference](/platform/reference/api/). Single-node behavior: [Behavior differences](/platform/deviations). Numeric ceilings: [Limits](/platform/limits).

## Products

| Product | Worker API | Topology difference |
| --- | --- | --- |
| [Workers](/workers/) | Module Workers (`fetch` / `scheduled` / `queue`) | Single-node `workerd`; no global edge |
| [KV](/kv/) | `KVNamespace` | Local SQLite; no global replication |
| [R2](/r2/) | `R2Bucket` | Object bytes on the configured S3; no global placement |
| [D1](/d1/) | `D1Database` | Local SQLite primary; no read replicas or region routing |
| [Durable Objects](/durable-objects/) | `DurableObject` / `DurableObjectNamespace` | Placed on the single local workerd process |
| [Alarms](/durable-objects/alarms) | `state.storage.setAlarm` | Local scheduler |
| [Queues](/queues/) | `Queue` and consumer handlers | Local `scheduler.sqlite`; at-least-once, no global FIFO |
| [Cron](/workers/configuration/cron-triggers) | `scheduled` handler | UTC, five fields; recovery projects at most the newest misfire |
| [Workflows](/workflows/) | Workflows API | Local SQLite; callbacks are at-least-once until commit |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` | Single-node cache |
| [Workers Cache](/workers/cache/) | Automatic HTTP cache | Single node; requires explicit `s-maxage` / `max-age` |
| [Static Assets](/workers/static-assets/) | Assets binding | Immutable S3 content served locally; no global CDN |
| [Service Bindings](/workers/runtime-apis/bindings) | `Fetcher` | Same platform; no cross-region discovery |
| [Deployments](/workers/versions-and-deployments/) | Versions, promotion, rollback | Local SQLite and one runtime generation |
| [Images](/images/) | Images binding | Bounded local raster transforms; not hosted Cloudflare Images |
| [Version Metadata](/workers/runtime-apis/bindings) | Deployment `id` / `tag` / `timestamp` | Produced by local deploy authority |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | Hibernatable WebSockets | On the local Durable Object process |

D1 covers database / session / prepared statement / result / meta, errors and bind conversions, atomic batches, and opaque bookmarks. General TCP outbound uses the one public Network and stock workerd's `cloudflare:sockets` / Node socket implementation; named Service / DO `Fetcher.connect()` uses an explicit capability tunnel.

## Runtime

The platform freezes the compatibility date. `open-compute.json` cannot set `compatibilityDate` or flags. Use `runtime.effective_compatibility_date`. Do not swap a workerd beside the binary, search `PATH`, or download another runtime.

Products that are not provided: [Unsupported](/platform/unsupported). Operators can inspect the running binary with `ocd capabilities --json`.

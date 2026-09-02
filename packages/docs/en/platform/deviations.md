# Behavior differences

The Worker API matches [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). The differences below come from single-node topology and the pinned `workerd`. Cloudflare products that are not provided: [Unsupported](/en/platform/unsupported).

## Workers

| Topic | Behavior | Docs |
| --- | --- | --- |
| Outbound `fetch` / sockets / `node:net` | Shared stock workerd `Network(allow=["public"])` | [TCP sockets](/en/workers/runtime-apis/tcp-sockets) |
| Named Service/DO `Fetcher.connect` | Declared capability tunnel, not a second general outbound | |
| CF IP-range block / self-connect detector / SMTP 25 | Not provided | |
| Runtime / binding-backend / workerd-internal listeners | Loopback | |
| Control / data listeners | Default loopback; an operator may expose them | |
| Exposed ingress and extra public-IP / SMTP egress policy | Operator-owned | |
| Request-scoped CPU / subrequest / simultaneous-connection quotas | Pinned open-source workerd standalone process does not enforce | [Workers limits](/en/workers/platform/limits) |
| `LimitEnforcer` subrequest accounting | No-op; `getLimitsExceeded()` always reports none | |
| Ineffective `WorkerLoader.ResourceLimits` | Not treated as enforced | |
| Public-address boundary, product durable limits, handle cleanup, process supervision | Still apply | |
| Other numeric ceilings | [Limits](/en/platform/limits) | |
| Deploy authority | One local SQLite authority and one supervised runtime generation | [Versions and deployments](/en/workers/versions-and-deployments/) |
| Global rollout / placement / traffic splitting / account management / billing control plane | Not provided | |
| Static Assets | Immutable S3-backed deployment content on the single-node platform; routing and bindings covered | [Static Assets](/en/workers/static-assets/) |
| Global CDN placement / replication / purge propagation / product quotas | Not provided | |
| Service Bindings | Default / named fetch and RPC within one platform authority | [Bindings](/en/workers/runtime-apis/bindings) |
| Cross-region placement / global service discovery | Not provided | |
| Target admission, deployment pins, capability lifetime, recovery | Local and fail closed | |
| Cron | UTC only, five fields, plus documented local Quartz-like extensions | [Cron Triggers](/en/workers/configuration/cron-triggers) |
| Cron recovery | At most the newest slot within misfire grace; no full downtime replay | |
| Cron known failures | Configured bounded local retry unless `noRetry()` is called | |

## Storage

| Topic | Behavior | Docs |
| --- | --- | --- |
| KV topology | Single-node SQLite authority; no global replication or edge-cache propagation timing | [KV](/en/kv/) |
| R2 | Object bytes held by the configured S3-compatible provider | [R2](/en/r2/) |
| R2 global placement / replication | Not provided | |
| D1 topology | Single local-primary SQLite authority | [D1](/en/d1/) |
| D1 read-replica / region routing | Not provided | |
| D1 `served_by` / region / colo metadata / billing counters | Not provided | |
| D1 bookmarks | Opaque bookmarks preserve same-database local sequential visibility | |
| D1 `rows_read` / `rows_written` | Stable local SQLite execution counters | |
| D1 `dump()` | Current hosted non-alpha `dump()` is rejected | |

## Compute

| Topic | Behavior | Docs |
| --- | --- | --- |
| Durable Object placement | Placed on the single local workerd process | [Durable Objects](/en/durable-objects/) |
| Location hints / jurisdiction / global migration | No geographic scheduling effect | |
| Queues durability | Single-node `scheduler.sqlite`; at-least-once delivery | [Queues](/en/queues/) |
| Queues global FIFO | Not provided | |
| Queues unknown native dispatch | Retains lease and does not consume tenant retry budget; later delivery can repeat the same attempt number | |
| Workflows execution | Local SQLite authority | [Workflows](/en/workflows/) |
| Workflows callbacks | At-least-once until result commits; replay skips durably completed callbacks | |
| Workflows external effects | Do not roll back with Workflow snapshots | |
| Workflows cross-region execution / global placement / dashboard / observability | Not provided | |

## Media

| Topic | Behavior | Docs |
| --- | --- | --- |
| Cache authority | Workers Cache and Cache API are single-node local authority | [Workers Cache](/en/workers/cache/), [Cache API](/en/workers/runtime-apis/cache) |
| Automatic caching | Requires explicit `s-maxage` or `max-age` | |
| Heuristic TTL / global replication / purge propagation | Not provided | |
| Tiered cache / Cache Rules / Cache Deception Armor / plan-dependent behavior | Not provided | |
| Default object size | 16 MiB per cached object; 1 GiB logical body bytes per Worker | |
| Live values | `ocd capabilities --json`; see [Limits](/en/platform/limits) | |
| Images | Bounded local raster transform binding, not hosted Cloudflare Images | [Images](/en/images/) |
| Hosted delivery / upload / signing, URL transforms, video, AI upscale, product quotas | Out of scope | |

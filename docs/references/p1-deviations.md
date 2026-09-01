# Cloudflare compatibility deviations

This file owns the stable deviation identifiers emitted by `ocd capabilities --json`.
It records verified single-node topology or pinned stock-runtime capacity differences, not a claim that unlisted
Cloudflare behavior is supported.
Functional gaps are blocked inventory members, never deviation IDs. The current Day1 inventory has no blocked target
member: Workers, KV, R2, D1, Durable Objects, Alarms, Queues, Cron, Workflows, Cache API, Version Metadata, and
hibernatable WebSockets are qualified against the pinned stable surface. `OC-WKR-TCP-001` and
`OC-WKR-LIMIT-001` record only the reviewed properties that this self-host contract cannot reproduce from
Cloudflare's hosted edge. D1 bookmarks, Durable Object hibernation/output gates, Queue `v8`/metadata, and Workflow
batch/rollback/structured-clone/parallel are implemented behavior, not deviations. P3 conformance audits every
advertised capability against this registry.

- `OC-WKR-TCP-001`: Tenant general outbound `fetch()`, `cloudflare:sockets.connect()`, and `node:net` share one stock-workerd `Network(allow = ["public"])` address authority. Named Service/DO `Fetcher.connect()` uses an explicitly declared capability tunnel and is not a second general outbound path. Unlike [Cloudflare's hosted TCP policy](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/#troubleshooting), open-compute does not add Cloudflare-owned IP-range blocking, a Worker self-connect/TCP-loop detector, or the default SMTP port 25 prohibition. Runtime-source, binding-backend, and workerd-internal listeners are loopback-only. Control/data listeners default to loopback but an operator may explicitly expose them, so the public Network does not add an ownership-based rejection for such public addresses. The operator owns exposed ingress and any additional public-IP, reverse-proxy, or SMTP egress policy.
- `OC-WKR-LIMIT-001`: The pinned stock OSS workerd standalone server's [`LimitEnforcer`](../../references/workerd/src/workerd/server/server.c++) explicitly enforces no limits: subrequest accounting is a no-op and `getLimitsExceeded()` always reports none. open-compute therefore does not claim [Cloudflare's request-scoped CPU, subrequest, or simultaneous-connection quotas](https://developers.cloudflare.com/workers/platform/limits/) and does not pass ineffective `WorkerLoader.ResourceLimits` as if they were enforced. This capacity deviation does not relax the public-address security boundary, product-specific durable limits, handle cleanup, or process supervision.
- `OC-KV-001`: KV is single-node SQLite authority; it does not claim Cloudflare global replication or edge-cache propagation timing.
- `OC-DEPLOY-001`: Deployments, routes, promotion, and rollback use one local SQLite authority and one supervised runtime generation. The platform does not claim Cloudflare's global rollout, placement, traffic-splitting, account-management, or billing control planes.
- `OC-ASSETS-001`: Static Assets are immutable S3-backed deployment content served by the single-node platform. Routing and binding behavior are covered, but Cloudflare's global CDN placement, replication, purge propagation, and product quotas are not provided.
- `OC-SERVICE-001`: Service Bindings provide default/named fetch and RPC within one platform authority. They do not claim Cloudflare cross-region placement or global service discovery; target admission, deployment pins, capability lifetime, and recovery are local and fail closed.
- `OC-R2-001`: R2 object bytes are held by the configured S3-compatible provider. The platform does not claim Cloudflare global placement or replication.
- `OC-D1-001`: D1 is a single local-primary SQLite authority. The platform does not claim read-replica/region routing,
  hosted `served_by` identity, region/colo metadata, or Cloudflare billing counters. Opaque bookmarks preserve same-database
  local sequential visibility; `rows_read` and `rows_written` are stable local SQLite execution counters.
- `OC-DO-001`: Durable Objects are placed on the single local workerd process. Location hints, jurisdiction, and global migration have no geographic scheduling effect.
- `OC-QUEUE-001`: Queue producers and push consumers are backed by single-node `scheduler.sqlite` durability, not Cloudflare global replication. Delivery is at-least-once without global FIFO. An unknown native dispatch retains its lease and does not consume the tenant retry budget, so a later delivery can repeat the same attempt number.
- `OC-CRON-001`: Cron is UTC-only with five fields and the documented local Quartz-like extensions. Recovery projects at most the newest slot within the configured misfire grace rather than replaying complete downtime history; known failures use the configured bounded local retry policy unless `noRetry()` is called.
- `OC-WORKFLOW-001`: Workflow execution uses local SQLite authority. Callbacks are at-least-once until their result commits; replay skips durably completed callbacks, and external product effects do not roll back with Workflow snapshots. The platform does not claim cross-region execution, global placement, or Cloudflare dashboard/observability.
- `OC-CACHE-001`: Workers Cache and Cache API are single-node local authority. Automatic caching requires an explicit `s-maxage` or `max-age`; heuristic TTL, global replication/purge propagation, tiered cache, Cache Rules, Cache Deception Armor, and plan-dependent behavior are unsupported.
- `OC-CACHE-002`: The operator-configured default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. The exact active values are emitted by `ocd capabilities --json`.
- `OC-IMAGES-001`: Images is a bounded local raster transform binding, not hosted Cloudflare Images. Hosted delivery/upload/signing, URL transforms, video, AI upscale, and Cloudflare product quotas are out of scope.

# Directory

open-compute provides the products below. The Worker API matches Cloudflare's docs; the topology is a single node. See [Behavior differences](/en/platform/deviations). Live limits come from `ocd capabilities --json`.

| Product | Description |
| --- | --- |
| [Workers](/en/workers/) | Module Workers on local `workerd` |
| [KV](/en/kv/) | Low-latency key-value storage |
| [D1](/en/d1/) | SQL |
| [R2](/en/r2/) | Object storage |
| [Durable Objects](/en/durable-objects/) | Stateful compute with strongly consistent storage |
| [Alarms](/en/durable-objects/alarms) | Timers inside a Durable Object (under DO) |
| [Queues](/en/queues/) | At-least-once delivery |
| [Cron](/en/workers/configuration/cron-triggers) | Scheduled Worker invocations (Workers config) |
| [Workflows](/en/workflows/) | Replayable multi-step applications |
| [Static Assets](/en/workers/static-assets/) | Immutable deployment static content |
| [Service Bindings](/en/workers/runtime-apis/bindings) | Worker-to-Worker calls on one platform |
| [Deployments](/en/workers/versions-and-deployments/) | Versions, promotion, rollback |
| [Workers Cache](/en/workers/cache/) | Automatic HTTP cache for Worker responses |
| [Cache API](/en/workers/runtime-apis/cache) | `caches.default` and friends |
| [Images](/en/images/) | Bounded local raster transforms |
| [Version Metadata](/en/workers/runtime-apis/bindings) | Immutable deployment `id` / `tag` / `timestamp` |
| [WebSocket hibernation](/en/workers/runtime-apis/websockets) | Hibernatable WebSockets on Durable Objects |

Alarms live under Durable Objects. Cron lives under Workers configuration. Platform notes: [Platform](/en/platform/). Worker API signatures: [API reference](/en/platform/reference/api/).

Cloudflare products that are not provided: [Unsupported](/en/platform/unsupported).

# Directory

open-compute provides the products below. The Worker API matches Cloudflare's docs; the topology is a single node. See [Behavior differences](/platform/deviations). Live limits come from `ocd capabilities --json`.

| Product | Description |
| --- | --- |
| [Workers](/workers/) | Module Workers on local `workerd` |
| [KV](/kv/) | Low-latency key-value storage |
| [D1](/d1/) | SQL |
| [R2](/r2/) | Object storage |
| [Durable Objects](/durable-objects/) | Stateful compute with strongly consistent storage |
| [Alarms](/durable-objects/alarms) | Timers inside a Durable Object (under DO) |
| [Queues](/queues/) | At-least-once delivery |
| [Cron](/workers/configuration/cron-triggers) | Scheduled Worker invocations (Workers config) |
| [Workflows](/workflows/) | Replayable multi-step applications |
| [Static Assets](/workers/static-assets/) | Immutable deployment static content |
| [Service Bindings](/workers/runtime-apis/bindings) | Worker-to-Worker calls on one platform |
| [Deployments](/workers/versions-and-deployments/) | Versions, promotion, rollback |
| [Workers Cache](/workers/cache/) | Automatic HTTP cache for Worker responses |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` and friends |
| [Images](/images/) | Bounded local raster transforms |
| [Vectorize](/vectorize/) | Stable post-beta vector index binding (exact search) |
| [AI Search](/ai-search/) | AI Search + Markdown Conversion via `env.AI` (operator-configured providers) |
| [Version Metadata](/workers/runtime-apis/bindings) | Immutable deployment `id` / `tag` / `timestamp` |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | Hibernatable WebSockets on Durable Objects |

Alarms live under Durable Objects. Cron lives under Workers configuration. Platform notes: [Platform](/platform/). Worker API signatures: [API reference](/platform/reference/api/).

Management surfaces (separate from product bindings): local Cloudflare v4 `/client/v4`, pinned Wrangler 4.127.1, and the operator / SDK-backed dashboard — see [Compatibility](/platform/compatibility).

Cloudflare products that are not provided: [Unsupported](/platform/unsupported).

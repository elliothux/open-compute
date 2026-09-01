# Directory

Trust `products` from `ocd capabilities --json`. This table is an inventory index, not a second source of truth. Cloudflare products that do not appear in the contract are unsupported.

| Product | One-liner | Status |
| --- | --- | --- |
| [Workers](/en/workers/) | Module Workers on local `workerd` | `supported_with_deviation` |
| [KV](/en/kv/) | Low-latency key-value storage | `supported_with_deviation` |
| [D1](/en/d1/) | SQL | `supported_with_deviation` |
| [R2](/en/r2/) | Object storage | `supported_with_deviation` |
| [Durable Objects](/en/durable-objects/) | Stateful compute with strongly consistent storage | `supported_with_deviation` |
| [Alarms](/en/durable-objects/alarms) | Timers inside a Durable Object (under DO) | `supported` |
| [Queues](/en/queues/) | At-least-once delivery | `supported_with_deviation` |
| [Cron](/en/workers/configuration/cron-triggers) | Scheduled Worker invocations (Workers config) | `supported_with_deviation` |
| [Workflows](/en/workflows/) | Replayable multi-step applications | `supported_with_deviation` |
| [Static Assets](/en/workers/static-assets/) | Immutable deployment static content | `supported_with_deviation` |
| [Service Bindings](/en/workers/runtime-apis/bindings) | Worker-to-Worker calls on one platform | `supported_with_deviation` |
| [Deployments](/en/workers/versions-and-deployments/) | Versions, promotion, rollback | `supported_with_deviation` |
| [Workers Cache](/en/workers/cache/) | Automatic HTTP cache for Worker responses | `supported_with_deviation` |
| [Cache API](/en/workers/runtime-apis/cache) | `caches.default` and friends | `supported_with_deviation` |
| [Images](/en/images/) | Bounded local raster transforms | `supported_with_deviation` |
| [Version Metadata](/en/workers/runtime-apis/bindings) | Immutable deployment `id` / `tag` / `timestamp` | `supported` |
| [WebSocket hibernation](/en/workers/runtime-apis/websockets) | Hibernatable WebSockets on Durable Objects | `supported` |

Alarms live under Durable Objects. Cron lives under Workers configuration. Platform contract hub: [Platform](/en/platform/). Member signatures: [generated API index](/en/platform/reference/api/). Deviation IDs: [Deviations](/en/platform/deviations).

Explicit non-target products (`non_target` / `unsupported`): [Unsupported](/en/platform/unsupported). Do not document them as products.

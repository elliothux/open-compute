---
title: "Directory"
---

open-compute provides the products below. The Worker API matches Cloudflare's docs; the topology is a single node. See [Behavior differences](/docs/platform/deviations). Live limits come from `ocd capabilities --json`.

| Product                                                        | Description                                                                  |
| -------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| [Workers](/docs/workers/)                                      | Module Workers on local `workerd`                                            |
| [KV](/docs/kv/)                                                | Low-latency key-value storage                                                |
| [D1](/docs/d1/)                                                | SQL                                                                          |
| [R2](/docs/r2/)                                                | Object storage                                                               |
| [Durable Objects](/docs/durable-objects/)                      | Stateful compute with strongly consistent storage                            |
| [Alarms](/docs/durable-objects/alarms)                         | Timers inside a Durable Object (under DO)                                    |
| [Queues](/docs/queues/)                                        | At-least-once delivery                                                       |
| [Cron](/docs/workers/configuration/cron-triggers)              | Scheduled Worker invocations (Workers config)                                |
| [Workflows](/docs/workflows/)                                  | Replayable multi-step applications                                           |
| [Static Assets](/docs/workers/static-assets/)                  | Immutable deployment static content                                          |
| [Service Bindings](/docs/workers/runtime-apis/bindings)        | Worker-to-Worker calls on one platform                                       |
| [Deployments](/docs/workers/versions-and-deployments/)         | Versions, promotion, rollback                                                |
| [Workers Cache](/docs/workers/cache/)                          | Automatic HTTP cache for Worker responses                                    |
| [Cache API](/docs/workers/runtime-apis/cache)                  | `caches.default` and friends                                                 |
| [Images](/docs/images/)                                        | Bounded local raster transforms                                              |
| [Vectorize](/docs/vectorize/)                                  | Stable post-beta vector index binding (exact search)                         |
| [AI Search](/docs/ai-search/)                                  | AI Search + Markdown Conversion via `env.AI` (operator-configured providers) |
| [Version Metadata](/docs/workers/runtime-apis/bindings)        | Immutable deployment `id` / `tag` / `timestamp`                              |
| [WebSocket hibernation](/docs/workers/runtime-apis/websockets) | Hibernatable WebSockets on Durable Objects                                   |

Alarms live under Durable Objects. Cron lives under Workers configuration. Platform notes: [Platform](/docs/platform/). Worker API signatures: [API reference](/docs/platform/reference/api/).

Management surfaces (separate from product bindings): local Cloudflare v4 `/client/v4`, pinned Wrangler 4.127.1, and the operator / SDK-backed dashboard — see [Compatibility](/docs/platform/compatibility).

Cloudflare products that are not provided: [Unsupported](/docs/platform/unsupported).

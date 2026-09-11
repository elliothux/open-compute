---
title: "API reference"
---

Worker signatures follow [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). This page lists shipped products and their docs. Differences from Cloudflare's hosted environment: [Behavior differences](/docs/platform/deviations).

| Product               | Docs                                                                   |
| --------------------- | ---------------------------------------------------------------------- |
| Workers               | [Workers](/docs/workers/), [Runtime APIs](/docs/workers/runtime-apis/) |
| KV                    | [KV](/docs/kv/)                                                        |
| R2                    | [R2](/docs/r2/)                                                        |
| D1                    | [D1](/docs/d1/)                                                        |
| Durable Objects       | [Durable Objects](/docs/durable-objects/)                              |
| Alarms                | [Alarms](/docs/durable-objects/alarms)                                 |
| Queues                | [Queues](/docs/queues/)                                                |
| Cron                  | [Cron triggers](/docs/workers/configuration/cron-triggers)             |
| Workflows             | [Workflows](/docs/workflows/)                                          |
| Cache API             | [Cache API](/docs/workers/runtime-apis/cache)                          |
| Workers Cache         | [Workers Cache](/docs/workers/cache/)                                  |
| Images                | [Images](/docs/images/)                                                |
| Version Metadata      | [Bindings](/docs/workers/runtime-apis/bindings)                        |
| WebSocket hibernation | [WebSockets](/docs/workers/runtime-apis/websockets)                    |

Release identity comes from `ocd capabilities --json` on the running binary.

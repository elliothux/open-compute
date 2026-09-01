# API reference

Worker signatures follow [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). This page lists shipped products and their docs. Differences from Cloudflare's hosted environment: [Behavior differences](/en/platform/deviations).

| Product | Docs |
| --- | --- |
| Workers | [Workers](/en/workers/), [Runtime APIs](/en/workers/runtime-apis/) |
| KV | [KV](/en/kv/) |
| R2 | [R2](/en/r2/) |
| D1 | [D1](/en/d1/) |
| Durable Objects | [Durable Objects](/en/durable-objects/) |
| Alarms | [Alarms](/en/durable-objects/alarms) |
| Queues | [Queues](/en/queues/) |
| Cron | [Cron triggers](/en/workers/configuration/cron-triggers) |
| Workflows | [Workflows](/en/workflows/) |
| Cache API | [Cache API](/en/workers/runtime-apis/cache) |
| Workers Cache | [Workers Cache](/en/workers/cache/) |
| Images | [Images](/en/images/) |
| Version Metadata | [Bindings](/en/workers/runtime-apis/bindings) |
| WebSocket hibernation | [WebSockets](/en/workers/runtime-apis/websockets) |

Release identity comes from `ocd capabilities --json` on the running binary.

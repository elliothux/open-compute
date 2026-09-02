# API reference

Worker signatures follow [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). This page lists shipped products and their docs. Differences from Cloudflare's hosted environment: [Behavior differences](/platform/deviations).

| Product | Docs |
| --- | --- |
| Workers | [Workers](/workers/), [Runtime APIs](/workers/runtime-apis/) |
| KV | [KV](/kv/) |
| R2 | [R2](/r2/) |
| D1 | [D1](/d1/) |
| Durable Objects | [Durable Objects](/durable-objects/) |
| Alarms | [Alarms](/durable-objects/alarms) |
| Queues | [Queues](/queues/) |
| Cron | [Cron triggers](/workers/configuration/cron-triggers) |
| Workflows | [Workflows](/workflows/) |
| Cache API | [Cache API](/workers/runtime-apis/cache) |
| Workers Cache | [Workers Cache](/workers/cache/) |
| Images | [Images](/images/) |
| Version Metadata | [Bindings](/workers/runtime-apis/bindings) |
| WebSocket hibernation | [WebSockets](/workers/runtime-apis/websockets) |

Release identity comes from `ocd capabilities --json` on the running binary.

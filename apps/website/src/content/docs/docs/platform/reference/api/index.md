---
title: "API and product index"
---

Worker signatures follow [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). This page links the shipped products and the machine-readable management surface. Differences from Cloudflare's hosted environment: [Behavior differences](/docs/platform/deviations/).

| Product               | Docs                                                                   |
| --------------------- | ---------------------------------------------------------------------- |
| Workers               | [Workers](/docs/workers/), [Runtime APIs](/docs/workers/runtime-apis/) |
| Dynamic Workers       | [Worker Loader](/docs/workers/runtime-apis/bindings/#dynamic-workers)  |
| Static Assets         | [Static Assets](/docs/workers/static-assets/)                          |
| Service Bindings      | [Bindings](/docs/workers/runtime-apis/bindings/)                       |
| Logs and live tail    | [Observability](/docs/workers/observability/)                          |
| KV                    | [KV](/docs/kv/)                                                        |
| R2                    | [R2](/docs/r2/)                                                        |
| D1                    | [D1](/docs/d1/)                                                        |
| Durable Objects       | [Durable Objects](/docs/durable-objects/)                              |
| Alarms                | [Alarms](/docs/durable-objects/alarms/)                                |
| Queues                | [Queues](/docs/queues/)                                                |
| Cron                  | [Cron triggers](/docs/workers/configuration/cron-triggers/)            |
| Workflows             | [Workflows](/docs/workflows/)                                          |
| Cache API             | [Cache API](/docs/workers/runtime-apis/cache/)                         |
| Workers Cache         | [Workers Cache](/docs/workers/cache/)                                  |
| Images                | [Images](/docs/images/)                                                |
| Vectorize             | [Vectorize](/docs/vectorize/)                                          |
| AI Search             | [AI Search](/docs/ai-search/)                                          |
| Artifacts             | [Artifacts](/docs/artifacts/)                                          |
| Version Metadata      | [Bindings](/docs/workers/runtime-apis/bindings/)                       |
| WebSocket hibernation | [WebSockets](/docs/workers/runtime-apis/websockets/)                   |

Release identity comes from `ocd capabilities --json` on the running binary.

## Management API and SDK

The Cloudflare-compatible management API is served under `/client/v4`. Use the [Management SDK](/docs/platform/reference/sdk/) for the typed TypeScript client. The committed `openapi/open-compute-sdk.json` and `packages/sdk/surface.json` files are the authoritative generated route and SDK inventories; this page is the human-readable product index.

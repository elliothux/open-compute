---
title: "API 与产品索引"
---

Worker 签名以 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 为准。本页连接已提供产品与 machine-readable 管理 surface。与托管环境的差异见[行为差异](/zh/docs/platform/deviations/)。

| 产品                  | 文档                                                                       |
| --------------------- | -------------------------------------------------------------------------- |
| Workers               | [Workers](/zh/docs/workers/)、[运行时 API](/zh/docs/workers/runtime-apis/) |
| Dynamic Workers       | [Worker Loader](/zh/docs/workers/runtime-apis/bindings/#dynamic-workers)   |
| Static Assets         | [Static Assets](/zh/docs/workers/static-assets/)                           |
| Service Bindings      | [Bindings](/zh/docs/workers/runtime-apis/bindings/)                        |
| 日志与实时 Tail       | [Observability](/zh/docs/workers/observability/)                           |
| KV                    | [KV](/zh/docs/kv/)                                                         |
| R2                    | [R2](/zh/docs/r2/)                                                         |
| D1                    | [D1](/zh/docs/d1/)                                                         |
| Durable Objects       | [Durable Objects](/zh/docs/durable-objects/)                               |
| Alarms                | [Alarms](/zh/docs/durable-objects/alarms/)                                 |
| Queues                | [Queues](/zh/docs/queues/)                                                 |
| Cron                  | [Cron 触发器](/zh/docs/workers/configuration/cron-triggers/)               |
| Workflows             | [Workflows](/zh/docs/workflows/)                                           |
| Cache API             | [Cache API](/zh/docs/workers/runtime-apis/cache/)                          |
| Workers Cache         | [Workers Cache](/zh/docs/workers/cache/)                                   |
| Images                | [Images](/zh/docs/images/)                                                 |
| Vectorize             | [Vectorize](/zh/docs/vectorize/)                                           |
| AI Search             | [AI Search](/zh/docs/ai-search/)                                           |
| Artifacts             | [Artifacts](/zh/docs/artifacts/)                                           |
| Version Metadata      | [Bindings](/zh/docs/workers/runtime-apis/bindings/)                        |
| WebSocket hibernation | [WebSockets](/zh/docs/workers/runtime-apis/websockets/)                    |

运行中的发行身份以 `ocd capabilities --json` 为准。

## 管理 API 与 SDK

Cloudflare-compatible 管理 API 位于 `/client/v4`。类型化 TypeScript client 见[管理 SDK](/zh/docs/platform/reference/sdk/)。仓库提交的 `openapi/open-compute-sdk.json` 与 `packages/sdk/surface.json` 是生成 route 与 SDK inventory 的 authority；本页仅作为面向人的产品索引。

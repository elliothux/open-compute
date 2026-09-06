# 产品目录

open-compute 提供下列产品。Worker API 与 Cloudflare 文档一致；数据与调度位于运行 `ocd` 的主机。差异见[行为差异](/zh/platform/deviations)。运行时限额以该主机上的 `ocd capabilities --json` 为准。

| 产品 | 说明 |
| --- | --- |
| [Workers](/zh/workers/) | 在本机 `workerd` 中运行模块 Worker |
| [KV](/zh/kv/) | 键值存储 |
| [D1](/zh/d1/) | SQL |
| [R2](/zh/r2/) | 对象存储 |
| [Durable Objects](/zh/durable-objects/) | 有状态对象，存储强一致 |
| [Alarms](/zh/durable-objects/alarms) | Durable Object 定时器 |
| [Queues](/zh/queues/) | Worker 间消息队列（at-least-once） |
| [Cron](/zh/workers/configuration/cron-triggers) | 按计划触发 Worker |
| [Workflows](/zh/workflows/) | 可从中断处恢复的多步工作流 |
| [Static Assets](/zh/workers/static-assets/) | 随部署发布的静态资源 |
| [Service Bindings](/zh/workers/runtime-apis/bindings) | 同一平台内的 Worker 互调 |
| [Deployments](/zh/workers/versions-and-deployments/) | 版本、发布与回滚 |
| [Workers Cache](/zh/workers/cache/) | Worker 响应的 HTTP 缓存 |
| [Cache API](/zh/workers/runtime-apis/cache) | `caches.default` 等 |
| [Images](/zh/images/) | 本地图像变换（受尺寸与并发限制） |
| [Vectorize](/zh/vectorize/) | 稳定后 beta 的向量索引 binding（精确检索） |
| [AI Search](/zh/ai-search/) | AI Search 与经 `env.AI` 的 Markdown Conversion（operator 配置的 provider） |
| [Version Metadata](/zh/workers/runtime-apis/bindings) | 部署的 `id` / `tag` / `timestamp` |
| [WebSocket hibernation](/zh/workers/runtime-apis/websockets) | Durable Object 上可休眠的 WebSocket |

Alarms 归入 Durable Objects。Cron 归入 Workers 配置。平台说明见[平台](/zh/platform/)。API 签名见 [API 参考](/zh/platform/reference/api/)。

管理面（与产品 binding 分开）：本地 Cloudflare v4 `/client/v4`、固定 Wrangler 4.127.1，以及 operator / SDK 支撑的 Dashboard — 见[兼容性](/zh/platform/compatibility)。

未提供的 Cloudflare 产品见[不支持](/zh/platform/unsupported)。

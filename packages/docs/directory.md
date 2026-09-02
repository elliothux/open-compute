# 产品目录

open-compute 提供下列产品。Worker API 与 Cloudflare 文档一致；数据与调度位于运行 `ocd` 的主机。差异见[行为差异](/platform/deviations)。运行时限额以该主机上的 `ocd capabilities --json` 为准。

| 产品 | 说明 |
| --- | --- |
| [Workers](/workers/) | 在本机 `workerd` 中运行模块 Worker |
| [KV](/kv/) | 键值存储 |
| [D1](/d1/) | SQL |
| [R2](/r2/) | 对象存储 |
| [Durable Objects](/durable-objects/) | 有状态对象，存储强一致 |
| [Alarms](/durable-objects/alarms) | Durable Object 定时器 |
| [Queues](/queues/) | Worker 间消息队列（at-least-once） |
| [Cron](/workers/configuration/cron-triggers) | 按计划触发 Worker |
| [Workflows](/workflows/) | 可从中断处恢复的多步工作流 |
| [Static Assets](/workers/static-assets/) | 随部署发布的静态资源 |
| [Service Bindings](/workers/runtime-apis/bindings) | 同一平台内的 Worker 互调 |
| [Deployments](/workers/versions-and-deployments/) | 版本、发布与回滚 |
| [Workers Cache](/workers/cache/) | Worker 响应的 HTTP 缓存 |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` 等 |
| [Images](/images/) | 本地图像变换（受尺寸与并发限制） |
| [Version Metadata](/workers/runtime-apis/bindings) | 部署的 `id` / `tag` / `timestamp` |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | Durable Object 上可休眠的 WebSocket |

Alarms 归入 Durable Objects。Cron 归入 Workers 配置。平台说明见[平台](/platform/)。API 签名见 [API 参考](/platform/reference/api/)。

未提供的 Cloudflare 产品见[不支持](/platform/unsupported)。

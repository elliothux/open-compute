# 产品目录

open-compute 提供下列产品。Worker 侧 API 与 Cloudflare 文档一致；拓扑是单节点。行为差异见[行为差异](/platform/deviations)。运行中的限制以 `ocd capabilities --json` 为准。

| 产品 | 说明 |
| --- | --- |
| [Workers](/workers/) | 模块 Worker，本机 `workerd` |
| [KV](/kv/) | 低延迟键值 |
| [D1](/d1/) | SQL |
| [R2](/r2/) | 对象存储 |
| [Durable Objects](/durable-objects/) | 有状态计算 + 强一致存储 |
| [Alarms](/durable-objects/alarms) | Durable Object 内的定时器（见 DO） |
| [Queues](/queues/) | 至少一次投递 |
| [Cron](/workers/configuration/cron-triggers) | Worker 定时触发（Workers 配置） |
| [Workflows](/workflows/) | 可重放的多步应用 |
| [Static Assets](/workers/static-assets/) | 不可变的部署静态内容 |
| [Service Bindings](/workers/runtime-apis/bindings) | 同一平台内的 Worker 互调 |
| [Deployments](/workers/versions-and-deployments/) | 版本、晋升、回滚 |
| [Workers Cache](/workers/cache/) | Worker 响应的自动 HTTP 缓存 |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` 等 |
| [Images](/images/) | 有界本机光栅变换 |
| [Version Metadata](/workers/runtime-apis/bindings) | 不可变的 deployment `id` / `tag` / `timestamp` |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | Durable Object 上可休眠的 WebSocket |

Alarms 记在 Durable Objects 下。Cron 记在 Workers 配置下。平台说明见[平台](/platform/)。Worker 侧签名见 [API 参考](/platform/reference/api/)。

未提供的 Cloudflare 产品见[不支持](/platform/unsupported)。

# 产品目录

以 `ocd capabilities --json` 的 `products` 为准。本表是库存索引，不是第二份真值。未出现在契约里的 Cloudflare 产品视为不支持。

| 产品 | 一句话 | 状态 |
| --- | --- | --- |
| [Workers](/workers/) | 模块 Worker，本机 `workerd` | `supported_with_deviation` |
| [KV](/kv/) | 低延迟键值 | `supported_with_deviation` |
| [D1](/d1/) | SQL | `supported_with_deviation` |
| [R2](/r2/) | 对象存储 | `supported_with_deviation` |
| [Durable Objects](/durable-objects/) | 有状态计算 + 强一致存储 | `supported_with_deviation` |
| [Alarms](/durable-objects/alarms) | Durable Object 内的定时器（见 DO） | `supported` |
| [Queues](/queues/) | 至少一次投递 | `supported_with_deviation` |
| [Cron](/workers/configuration/cron-triggers) | Worker 定时触发（Workers 配置） | `supported_with_deviation` |
| [Workflows](/workflows/) | 可重放的多步应用 | `supported_with_deviation` |
| [Static Assets](/workers/static-assets/) | 不可变的部署静态内容 | `supported_with_deviation` |
| [Service Bindings](/workers/runtime-apis/bindings) | 同一平台内的 Worker 互调 | `supported_with_deviation` |
| [Deployments](/workers/versions-and-deployments/) | 版本、晋升、回滚 | `supported_with_deviation` |
| [Workers Cache](/workers/cache/) | Worker 响应的自动 HTTP 缓存 | `supported_with_deviation` |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` 等 | `supported_with_deviation` |
| [Images](/images/) | 有界本机光栅变换 | `supported_with_deviation` |
| [Version Metadata](/workers/runtime-apis/bindings) | 不可变的 deployment `id` / `tag` / `timestamp` | `supported` |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | Durable Object 上可休眠的 WebSocket | `supported` |

Alarms 记在 Durable Objects 下。Cron 记在 Workers 配置下。平台契约入口见[平台](/platform/)。成员签名见[生成 API 索引](/platform/reference/api/)。差异 ID 见[偏差](/platform/deviations)。

明确非目标产品（`non_target` / `unsupported`）见[不支持](/platform/unsupported)，不要把它们写成产品页。

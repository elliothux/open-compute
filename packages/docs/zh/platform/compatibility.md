# 兼容性

open-compute 实现 Cloudflare Workers 中已支持的 API。已提供产品的 Worker 用法与 Cloudflare 文档一致。数据与进程位于单机：一个 `ocd`，一个锁定版本的 `workerd`。

运行中的限额与版本信息：

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` 对 `capabilities` 可选。省略时，`limits` 来自内嵌默认配置；指定绝对路径时，反映该配置文件。

Worker 签名以 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 为准，索引见 [API 参考](/zh/platform/reference/api/)。与托管环境的差异见[行为差异](/zh/platform/deviations)。数字上限见[限制](/zh/platform/limits)。

## 产品

| 产品 | Worker API | 数据 / 进程位置 |
| --- | --- | --- |
| [Workers](/zh/workers/) | 模块 Worker（`fetch` / `scheduled` / `queue`） | 本机 `workerd`；不提供全球边缘网络 |
| [KV](/zh/kv/) | `KVNamespace` | 本机 SQLite；不提供全球复制 |
| [R2](/zh/r2/) | `R2Bucket` | 对象位于选定的 Local 或 S3 authority；不提供全球就近存放 |
| [D1](/zh/d1/) | `D1Database` | 本机一份 SQLite；不提供只读副本与按区域路由 |
| [Durable Objects](/zh/durable-objects/) | `DurableObject` / `DurableObjectNamespace` | 本机单个 workerd 进程 |
| [Alarms](/zh/durable-objects/alarms) | `state.storage.setAlarm` | 本机调度 |
| [Queues](/zh/queues/) | `Queue` 与消费者 `queue` | 本机 `scheduler.sqlite`；at-least-once，不提供全局 FIFO |
| [Cron](/zh/workers/configuration/cron-triggers) | `scheduled` | UTC 五字段；错过触发后最多补最近一次 |
| [Workflows](/zh/workflows/) | Workflows API | 本机 SQLite；步骤回调在提交前可能重复执行 |
| [Cache API](/zh/workers/runtime-apis/cache) | `caches.default` | 仅本机缓存 |
| [Workers Cache](/zh/workers/cache/) | 自动 HTTP 缓存 | 本机；需要显式 `s-maxage` / `max-age` |
| [Static Assets](/zh/workers/static-assets/) | assets 绑定 | 随部署存放于 Local/S3 authority；不是全球 CDN |
| [Service Bindings](/zh/workers/runtime-apis/bindings) | `Fetcher` | 同一平台内；不提供跨地区发现 |
| [Deployments](/zh/workers/versions-and-deployments/) | 版本、发布与回滚 | 本机 SQLite；`ocd` 监督当前 workerd 进程 |
| [Images](/zh/images/) | Images 绑定 | 本机图像变换；不是托管 Cloudflare Images |
| [Vectorize](/zh/vectorize/) | 稳定后 beta 的 `Vectorize` | 本机精确检索；每索引一份 SQLite；beta `VectorizeIndex` 不在范围 |
| [AI Search](/zh/ai-search/) | `env.AI` Markdown Conversion 与 AI Search | operator 配置的 OpenAI-compatible provider；不提供完整 Workers AI 推理 |
| [Version Metadata](/zh/workers/runtime-apis/bindings) | 部署的 `id` / `tag` / `timestamp` | 由本机本次部署生成 |
| [WebSocket hibernation](/zh/workers/runtime-apis/websockets) | 可休眠 WebSocket | 本机 Durable Object 进程 |

D1 覆盖 database / session / prepared statement / result / meta、错误与 bind 转换、原子 batch，以及不透明 bookmark。通用 TCP 出站使用唯一的 public Network，以及开源 workerd 的 `cloudflare:sockets` / Node socket；命名 Service / DO 的 `Fetcher.connect()` 使用绑定声明的连接。

## 运行时

兼容日期由平台锁定，`wrangler.jsonc` 不得设置 `compatibilityDate` 或 flags。以 `runtime.effective_compatibility_date` 为准。不要替换二进制旁的 workerd，也不要从 `PATH` 另行解析 runtime。

## 管理面

管理面与产品 binding 分开：

| 表面 | 状态 |
| --- | --- |
| Cloudflare v4 API | █████████░ 90% — 本地 `/client/v4` 可与 Wrangler 及官方 SDK 配合使用。与 Cloudflare 托管端逐字段对照仍需要 Cloudflare 账号凭证。 |
| Wrangler | █████████░ 95% — 固定 Wrangler `4.127.1`：部署与资源命令已在运行中的 `ocd` 上验证。 |
| Dashboard | ████████░░ 80% — 基于同一套 `/client/v4` API 的 operator 管理界面，不是 Cloudflare Dashboard 的克隆。 |
| Workers Logs / realtime tail | █████████░ 90% — 单机支持 `wrangler tail` 以及 Workers Logs 查询与 live tail。不提供 Tail Workers、分布式 traces、Logpush。 |

## 部分支持 / 规划中 / 尚未支持

| 模块 | 状态 |
| --- | --- |
| [Vectorize](/zh/vectorize/) | ████████░░ 80% — 部分支持：稳定后 beta 的 `Vectorize` API；beta `VectorizeIndex` 不在范围。 |
| Markdown Conversion | ████████░░ 80% — 部分支持：经标准 `env.AI`（`toMarkdown`）。见 [AI Search](/zh/ai-search/)。 |
| [AI Search](/zh/ai-search/) | ████████░░ 80% — 部分支持：由 operator 配置 OpenAI-compatible provider 的 RAG。 |
| Browser Run | ██░░░░░░░░ 20% — 规划中（原 Browser Rendering）。 |
| Artifacts | ██░░░░░░░░ 20% — 规划中。 |
| Workers AI | ░░░░░░░░░░ 0% — 尚未支持：不提供托管模型推理。 |
| Containers | ░░░░░░░░░░ 0% — 尚未支持。 |
| Hyperdrive | ░░░░░░░░░░ 0% — 尚未支持。 |
| Analytics Engine | ░░░░░░░░░░ 0% — 尚未支持。 |
| Workers for Platforms | ░░░░░░░░░░ 0% — 尚未支持。 |
| Dynamic Workers | ░░░░░░░░░░ 0% — 尚未支持。 |
| Pipelines | ░░░░░░░░░░ 0% — 尚未支持。 |
| Rate Limiting | ░░░░░░░░░░ 0% — 尚未支持。 |
| mTLS certificates | ░░░░░░░░░░ 0% — 尚未支持。 |
| Tail Workers / traces / Logpush | ░░░░░░░░░░ 0% — 尚未支持。 |

完整列表与配置字段名见[不支持](/zh/platform/unsupported)。运行中表面：`ocd capabilities --json`。

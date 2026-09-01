# 兼容性

open-compute 实现 Cloudflare Workers 中已支持的 API。已提供产品的 Worker 用法与 Cloudflare 文档一致。数据与进程位于单机：一个 `ocd`，一个锁定版本的 `workerd`。

运行中的限额与版本信息：

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` 对 `capabilities` 可选。省略时，`limits` 来自内嵌默认配置；指定绝对路径时，反映该配置文件。

Worker 签名以 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 为准，索引见 [API 参考](/platform/reference/api/)。与托管环境的差异见[行为差异](/platform/deviations)。数字上限见[限制](/platform/limits)。

## 产品

| 产品 | Worker API | 数据 / 进程位置 |
| --- | --- | --- |
| [Workers](/workers/) | 模块 Worker（`fetch` / `scheduled` / `queue`） | 本机 `workerd`；不提供全球边缘网络 |
| [KV](/kv/) | `KVNamespace` | 本机 SQLite；不提供全球复制 |
| [R2](/r2/) | `R2Bucket` | 对象位于配置的 S3；不提供全球就近存放 |
| [D1](/d1/) | `D1Database` | 本机一份 SQLite；不提供只读副本与按区域路由 |
| [Durable Objects](/durable-objects/) | `DurableObject` / `DurableObjectNamespace` | 本机单个 workerd 进程 |
| [Alarms](/durable-objects/alarms) | `state.storage.setAlarm` | 本机调度 |
| [Queues](/queues/) | `Queue` 与消费者 `queue` | 本机 `scheduler.sqlite`；at-least-once，不提供全局 FIFO |
| [Cron](/workers/configuration/cron-triggers) | `scheduled` | UTC 五字段；错过触发后最多补最近一次 |
| [Workflows](/workflows/) | Workflows API | 本机 SQLite；步骤回调在提交前可能重复执行 |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` | 仅本机缓存 |
| [Workers Cache](/workers/cache/) | 自动 HTTP 缓存 | 本机；需要显式 `s-maxage` / `max-age` |
| [Static Assets](/workers/static-assets/) | assets 绑定 | 随部署存放于本机使用的 S3；不是全球 CDN |
| [Service Bindings](/workers/runtime-apis/bindings) | `Fetcher` | 同一平台内；不提供跨地区发现 |
| [Deployments](/workers/versions-and-deployments/) | 版本、发布与回滚 | 本机 SQLite；`ocd` 监督当前 workerd 进程 |
| [Images](/images/) | Images 绑定 | 本机图像变换；不是托管 Cloudflare Images |
| [Version Metadata](/workers/runtime-apis/bindings) | 部署的 `id` / `tag` / `timestamp` | 由本机本次部署生成 |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | 可休眠 WebSocket | 本机 Durable Object 进程 |

D1 覆盖 database / session / prepared statement / result / meta、错误与 bind 转换、原子 batch，以及不透明 bookmark。通用 TCP 出站使用唯一的 public Network，以及开源 workerd 的 `cloudflare:sockets` / Node socket；命名 Service / DO 的 `Fetcher.connect()` 使用绑定声明的连接。

## 运行时

兼容日期由平台锁定，`open-compute.json` 不得设置 `compatibilityDate` 或 flags。以 `runtime.effective_compatibility_date` 为准。不要替换二进制旁的 workerd，也不要从 `PATH` 另行解析 runtime。

未提供的产品见[不支持](/platform/unsupported)。运维可通过 `ocd capabilities --json` 查看运行中的二进制。

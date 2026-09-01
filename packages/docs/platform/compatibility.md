# 兼容性

open-compute 实现已声明的 Cloudflare Workers 编程模型。每个已提供产品的 Worker API 与 Cloudflare 文档一致；拓扑是单节点（一个 `ocd`、一个固定版本的 `workerd`）。

运行中的限制与发行身份：

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` 对 `capabilities` 是可选的。省略时，`limits` 来自内嵌默认配置；给出绝对路径配置时，`limits` 反映该文件。

Worker 侧签名以 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 为准，索引见 [API 参考](/platform/reference/api/)。单节点行为见[行为差异](/platform/deviations)。数字上限见[限制](/platform/limits)。

## 产品

| 产品 | Worker API | 拓扑差异 |
| --- | --- | --- |
| [Workers](/workers/) | 模块 Worker（`fetch` / `scheduled` / `queue`） | 单节点 `workerd`；无全球边缘 |
| [KV](/kv/) | `KVNamespace` | 本机 SQLite；无全球复制 |
| [R2](/r2/) | `R2Bucket` | 对象字节在配置的 S3；无全球放置 |
| [D1](/d1/) | `D1Database` | 本机 SQLite 主库；无只读副本与地域路由 |
| [Durable Objects](/durable-objects/) | `DurableObject` / `DurableObjectNamespace` | 落在本地这一个 workerd 进程 |
| [Alarms](/durable-objects/alarms) | `state.storage.setAlarm` | 本机调度 |
| [Queues](/queues/) | `Queue` 与 consumer handler | 本机 `scheduler.sqlite`；至少一次，无全球 FIFO |
| [Cron](/workers/configuration/cron-triggers) | `scheduled` handler | UTC、五个字段；恢复最多补最近一个 misfire |
| [Workflows](/workflows/) | Workflows API | 本机 SQLite；callback 在提交前至少一次 |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` | 单节点缓存 |
| [Workers Cache](/workers/cache/) | 自动 HTTP 缓存 | 单节点；需显式 `s-maxage` / `max-age` |
| [Static Assets](/workers/static-assets/) | assets binding | 本机提供的不可变 S3 内容；无全球 CDN |
| [Service Bindings](/workers/runtime-apis/bindings) | `Fetcher` | 同一平台内；无跨地域发现 |
| [Deployments](/workers/versions-and-deployments/) | 版本、晋升、回滚 | 本机 SQLite 与一个 runtime generation |
| [Images](/images/) | Images binding | 本机有界光栅变换；非托管 Cloudflare Images |
| [Version Metadata](/workers/runtime-apis/bindings) | deployment `id` / `tag` / `timestamp` | 由本机部署权威生成 |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | 可休眠 WebSocket | 本机 Durable Object 进程 |

D1 覆盖 database / session / prepared statement / result / meta、错误与 bind 转换、原子 batch 以及不透明 bookmark。通用 TCP 出站使用唯一的 public Network 以及 stock workerd 的 `cloudflare:sockets` / Node socket；命名 Service / DO 的 `Fetcher.connect()` 走显式 capability tunnel。

## 运行时

compatibility date 由平台冻结，`open-compute.json` 不能设置 `compatibilityDate` 或 flags。以 `runtime.effective_compatibility_date` 为准。不要替换二进制旁的 workerd，也不要从 `PATH` 搜索或另下 runtime。

未提供的产品见[不支持](/platform/unsupported)。运维可以用 `ocd capabilities --json` 查看正在运行的二进制。

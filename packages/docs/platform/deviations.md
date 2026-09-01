# 行为差异

Worker API 与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 一致。下列差异源于单机部署以及锁定版本的开源 `workerd`。未提供的 Cloudflare 产品见[不支持](/platform/unsupported)。

## Workers

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| 出站 `fetch` / sockets / `node:net` | 共用开源 workerd 的 `Network(allow=["public"])` | [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| 命名 Service / DO 的 `Fetcher.connect` | 使用绑定声明的连接，而非第二条通用出站通道 | |
| Cloudflare 自有 IP 封禁、自连接检测、默认 SMTP 25 拦截 | 不提供 | |
| workerd 内部监听 | 绑定 loopback | |
| 控制面 / 数据面监听 | 默认 loopback，运维可改为对外暴露 | |
| 公网入口与 SMTP 出站策略 | 由运维负责 | |
| 单请求 CPU / 子请求 / 并发连接配额 | 该开源 workerd 不执行 Cloudflare 托管环境的配额 | [Workers 限制](/workers/platform/limits) |
| 子请求计数 | 不计数；`getLimitsExceeded()` 始终报告未超限 | |
| `WorkerLoader.ResourceLimits` | 设置后不会作为已执行的限制 | |
| 公网地址边界、存储限额、句柄清理、进程监督 | 仍然有效 | |
| 其他数字上限 | 见[限制](/platform/limits) | |
| 部署状态 | 本机 SQLite；`ocd` 监督当前 workerd 进程 | [版本与部署](/workers/versions-and-deployments/) |
| 全球灰度、就近放置、流量拆分、账号与计费 | 不提供 | |
| 静态资源 | 随部署存放于本机使用的 S3 | [静态资源](/workers/static-assets/) |
| 全球 CDN、复制、purge 传播、CDN 配额 | 不提供 | |
| Service Bindings | 同一平台内的 fetch 与 RPC | [Bindings](/workers/runtime-apis/bindings) |
| 跨地区放置、全球服务发现 | 不提供 | |
| 调用方准入与部署钉扎 | 在本机判定；失败则关闭 | |
| Cron | UTC 五字段，以及文档所述本机扩展 | [Cron 触发器](/workers/configuration/cron-triggers) |
| 错过的 Cron | 宽限时间内最多补最近一次，不回放停机期间的全部触发 | |
| Cron 失败重试 | 按配置有限次重试；调用 `noRetry()` 除外 | |

## 存储

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| KV | 本机 SQLite；不提供全球复制或边缘缓存 | [KV](/kv/) |
| R2 对象 | 存放在配置的 S3 兼容存储 | [R2](/r2/) |
| R2 全球就近存放 / 复制 | 不提供 | |
| D1 | 本机一份 SQLite | [D1](/d1/) |
| D1 只读副本 / 按区域路由 | 不提供 | |
| D1 `served_by` / 地域 / colo / 计费计数 | 不提供 | |
| D1 bookmark | 不透明 token，保证同一数据库上的顺序 | |
| D1 `rows_read` / `rows_written` | 本地 SQLite 执行计数 | |
| D1 `dump()` | 与托管非 alpha 相同，拒绝该接口 | |

## 计算

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| Durable Objects 位置 | 本机单个 workerd 进程 | [Durable Objects](/durable-objects/) |
| location hint / jurisdiction / 全球迁移 | 不产生地理调度效果 | |
| Queues 存储 | 本机 `scheduler.sqlite`；投递语义为 at-least-once | [Queues](/queues/) |
| Queues 全局 FIFO | 不提供 | |
| 无法识别的 native dispatch | 可能保留消息 lease；后续投递可能使用同一 attempt 编号 | |
| Workflows | 步骤状态位于本机 SQLite | [Workflows](/workflows/) |
| Workflows 步骤回调 | 结果提交前可能重复执行；已持久化的步骤在重放时跳过 | |
| Workflows 外部副作用 | 不随快照回滚 | |
| Workflows 跨地区执行、控制台与可观测性 | 不提供 | |

## 媒体

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| Cache | Workers Cache 与 Cache API 仅在本机 | [Workers Cache](/workers/cache/)、[Cache API](/workers/runtime-apis/cache) |
| 自动缓存 | 需要显式 `s-maxage` 或 `max-age` | |
| 启发式 TTL、全球复制、purge 传播 | 不提供 | |
| 分层缓存、Cache Rules、Cache Deception Armor、按套餐行为 | 不提供 | |
| 默认大小 | 每对象 16 MiB；每 Worker 逻辑 body 1 GiB | |
| 运行中的精确值 | `ocd capabilities --json`，见[限制](/platform/limits) | |
| Images | 对本机请求体中的图像做变换，不是 Cloudflare 托管 Images | [Images](/images/) |
| 图像库、上传、签名、URL 变换、视频、AI 放大、托管配额 | 不提供 | |

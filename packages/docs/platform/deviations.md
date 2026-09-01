# 行为差异

Worker 侧 API 与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 一致。下列差异来自单节点拓扑与固定版本的 `workerd`。未提供的 Cloudflare 产品见[不支持](/platform/unsupported)。

## Workers

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| 出站 `fetch` / sockets / `node:net` | 共用 stock workerd `Network(allow=["public"])` | [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| 命名 Service/DO `Fetcher.connect` | 声明式 capability tunnel，不是第二条通用出站 | |
| CF IP 段封禁 / self-connect 检测 / SMTP 25 | 不提供 | |
| runtime / binding 后端 / workerd 内部 listener | 固定 loopback | |
| control / data listener | 默认 loopback，运维可显式对外暴露 | |
| 公开入口与额外公网 / SMTP egress 策略 | 由运维负责 | |
| request-scoped CPU / subrequest / simultaneous-connection 配额 | 固定版本开源 workerd 独立进程不执行 | [Workers limits](/workers/platform/limits) |
| `LimitEnforcer` subrequest 记账 | 空操作；`getLimitsExceeded()` 始终未超限 | |
| 无效的 `WorkerLoader.ResourceLimits` | 不当成已执行 | |
| 公网地址边界、产品耐久限额、handle 清理、进程监督 | 仍然有效 | |
| 其它数字上限 | 见[限制](/platform/limits) | |
| 部署权威 | 一份本地 SQLite 与一个受监督的 runtime generation | [版本与部署](/workers/versions-and-deployments/) |
| 全球 rollout / placement / 流量拆分 / 账号管理 / 计费控制面 | 不提供 | |
| Static Assets | 单节点上不可变的 S3 部署内容；路由和 binding 已覆盖 | [静态资源](/workers/static-assets/) |
| 全球 CDN placement / 复制 / purge 传播 / 产品配额 | 不提供 | |
| Service Bindings | 同一平台权威内 default / named fetch 与 RPC | [Bindings](/workers/runtime-apis/bindings) |
| 跨地域 placement / 全球服务发现 | 不提供 | |
| 目标准入、deployment pin、capability 生命周期、恢复 | 本地，失败即关闭 | |
| Cron | UTC、五个字段，以及已文档化的本机 Quartz-like 扩展 | [Cron 触发器](/workers/configuration/cron-triggers) |
| Cron 恢复 | 最多投影 misfire grace 内最新的一个 slot，不重放完整停机历史 | |
| Cron 已知失败 | 配置里的有界本机重试，除非调用了 `noRetry()` | |

## Storage

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| KV 拓扑 | 单节点 SQLite 权威；没有全球复制或边缘缓存传播时延 | [KV](/kv/) |
| R2 | 对象字节由配置的 S3-compatible provider 持有 | [R2](/r2/) |
| R2 全球 placement / replication | 不提供 | |
| D1 拓扑 | 单个本地主 SQLite authority | [D1](/d1/) |
| D1 read replica / region routing | 不提供 | |
| D1 `served_by` / region / colo metadata / 计费计数 | 不提供 | |
| D1 bookmark | opaque bookmark 保证同一数据库的本地顺序可见性 | |
| D1 `rows_read` / `rows_written` | 稳定的本地 SQLite 执行计数 | |
| D1 `dump()` | 当前拒绝 hosted 非 alpha 的 `dump()` | |

## Compute

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| Durable Objects 放置 | 落在本地这一个 workerd 进程 | [Durable Objects](/durable-objects/) |
| location hint / jurisdiction / 全球迁移 | 无地理调度效果 | |
| Queues 耐久性 | 单节点 `scheduler.sqlite`；投递 at-least-once | [Queues](/queues/) |
| Queues 全球 FIFO | 不提供 | |
| Queues 未知 native dispatch | 保留 lease，不消耗租户重试预算；后续投递可能重复同一 attempt number | |
| Workflows 执行 | 本地 SQLite authority | [Workflows](/workflows/) |
| Workflows callback | 结果提交前 at-least-once；replay 跳过已耐久完成的 callback | |
| Workflows 外部副作用 | 不随 Workflow snapshot 回滚 | |
| Workflows 跨地域执行 / 全球 placement / dashboard / observability | 不提供 | |

## Media

| 主题 | 行为 | 文档 |
| --- | --- | --- |
| Cache 权威 | Workers Cache 与 Cache API 为单节点本机权威 | [Workers Cache](/workers/cache/)、[Cache API](/workers/runtime-apis/cache) |
| 自动缓存 | 需要显式 `s-maxage` 或 `max-age` | |
| 启发式 TTL / 全球复制 / purge 传播 | 不提供 | |
| tiered cache / Cache Rules / Cache Deception Armor / 套餐行为 | 不提供 | |
| 默认对象大小 | 每对象 16 MiB；每 Worker 1 GiB 逻辑 body 字节 | |
| 运行中精确值 | `ocd capabilities --json`；见[限制](/platform/limits) | |
| Images | 有界的本机光栅变换 binding，不是托管的 Cloudflare Images | [Images](/images/) |
| 托管投递 / 上传 / 签名、URL transform、视频、AI upscale、产品配额 | 不在范围内 | |

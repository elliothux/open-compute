# 偏差

这些 `OC-*` ID 会出现在 `ocd capabilities --json` 对应产品的 `deviations` 里。含义以本页为准；没有 ID 不等于 Cloudflare 其它行为可用。功能缺口是 `blocked` 成员，从来不是 deviation ID。当前库存没有 `blocked` 目标成员。

## OC-WKR-TCP-001

tenant 的 general outbound `fetch()`、`cloudflare:sockets.connect()` 和 `node:net` 共享唯一的 stock-workerd `Network(allow = ["public"])` 地址权威。命名 Service/DO 的 `Fetcher.connect()` 走声明式 capability tunnel，不是第二条通用 outbound。

与 [Cloudflare 托管 TCP 策略](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/#troubleshooting) 不同：open-compute 不复制 Cloudflare 自有 IP 段封禁、Worker self-connect/TCP-loop detector 或默认 SMTP 25 封禁。runtime-source、binding-backend 和 workerd 内部 listener 强制 loopback。control/data listener 默认 loopback，但 operator 可以显式暴露，因此不能宣称 public Network 会按“平台所有权”额外拒绝公开地址。operator 负责公开入口和额外公网/SMTP egress policy。

## OC-WKR-LIMIT-001

pinned stock OSS workerd standalone 的 [`LimitEnforcer`](https://developers.cloudflare.com/workers/platform/limits/) 不执行限制：subrequest 记账是空操作，`getLimitsExceeded()` 永远报告无超限。open-compute 因此不声称 Cloudflare 托管环境的 request-scoped CPU、subrequest 或 simultaneous-connection quota，也不会把无效的 `WorkerLoader.ResourceLimits` 当成已执行。

这条容量偏差不放宽 public-address 安全边界、产品专有耐久限额、handle 清理和进程监督。运行中的其它数字上限见[限制](/platform/limits)。

## OC-KV-001

KV 是单节点 SQLite 权威存储，不声称 Cloudflare 全球复制或边缘缓存传播时延。

## OC-DEPLOY-001

部署、路由、晋升和回滚使用一份本地 SQLite 权威和一个受监督的 runtime generation。平台不声称 Cloudflare 的全球 rollout、placement、流量拆分、账号管理或计费控制面。

## OC-ASSETS-001

Static Assets 是由单节点平台提供的、不可变的、S3 上的部署内容。路由和 binding 行为已覆盖，但不提供 Cloudflare 全球 CDN placement、复制、purge 传播和产品配额。

## OC-SERVICE-001

Service Binding 在同一平台权威内提供 default/named fetch 与 RPC。不声称 Cloudflare 跨地域 placement 或全球服务发现；目标准入、deployment pin、capability 生命周期和恢复都是本地的，失败即关闭。

## OC-R2-001

R2 object bytes 由配置的 S3-compatible provider 持有。不声称 Cloudflare 全球 placement 或 replication。

## OC-D1-001

D1 是单个本地主 SQLite authority。不声称 read replica、region routing、hosted `served_by` 身份、region/colo metadata 或 Cloudflare 计费计数。opaque bookmark 保证同一数据库的本地顺序可见性；`rows_read` / `rows_written` 是稳定的本地 SQLite 执行计数。

## OC-DO-001

Durable Objects 落在本地这一个 workerd 进程上。location hint、jurisdiction 和全球迁移没有地理调度效果。

## OC-QUEUE-001

Queue producer 和 push consumer 的耐久性来自单节点 `scheduler.sqlite`，不是 Cloudflare 全球复制。投递是 at-least-once，没有全球 FIFO。未知的 native dispatch 会保留 lease，不消耗租户重试预算，所以后续投递可能重复同一 attempt number。

## OC-CRON-001

Cron 只有 UTC、五个字段，以及已文档化的本机 Quartz-like 扩展。恢复时最多投影 misfire grace 内最新的一个 slot，不会重放完整停机历史。已知失败走配置里的有界本机重试，除非调用了 `noRetry()`。

## OC-WORKFLOW-001

Workflow 在本地 SQLite authority 上执行。callback 在结果提交前是 at-least-once；replay 会跳过已耐久完成的 callback；外部产品副作用不会随 Workflow snapshot 回滚。不声称跨地域执行、全球 placement 或 Cloudflare dashboard/observability。

## OC-CACHE-001

Workers Cache 和 Cache API 是单节点本机权威。自动缓存需要显式 `s-maxage` 或 `max-age`。不支持启发式 TTL、全球复制/purge 传播、tiered cache、Cache Rules、Cache Deception Armor，以及依赖套餐的行为。

## OC-CACHE-002

运维配置的默认值是每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值由 `ocd capabilities --json` 给出，见[限制](/platform/limits)。

## OC-IMAGES-001

Images 是有界的本机光栅变换 binding，不是托管的 Cloudflare Images。托管投递/上传/签名、URL transform、视频、AI upscale 和 Cloudflare 产品配额不在范围内。

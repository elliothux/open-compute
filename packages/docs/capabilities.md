# 能力与限制

以这台机器上的二进制为准，不要用产品名字推导 Cloudflare 全量行为。未出现在 `capabilities --json` 里的 Cloudflare 功能视为不支持。本页不是一份完整 Cloudflare 矩阵。

```sh
platformd --config /etc/open-compute/config.toml capabilities --json
```

`--config` 对 `capabilities` 是可选的。省略时，`limits` 来自内嵌默认配置；给出绝对路径配置时，`limits` 反映该文件。JSON 顶层字段如下。

## schema_version

固定为 `1`。其它值不要当这份契约来读。

## release

精确的发行身份，不是营销版本号。包含 `platform_version`、`git_revision`、`rust_msrv`、`workerd_version`、`workerd_lock_sha256`、`runtime_assets_sha256`、`facade_capability_version`，以及 control / scheduler / KV / D1 的 schema 版本和 `snapshot_format_version`。恢复和替换二进制时，拿这里的身份去对快照与 schema，而不是看文件名。

## runtime

pinned workerd 的固定 baseline：

| 字段 | 含义 |
| --- | --- |
| `effective_compatibility_date` | 正式 runtime lock 的唯一生效 compatibility date |
| `workerd_lock_sha256` | 正式 runtime lock 字节的 SHA-256 |
| `workers_types_version` | 固定的 `@cloudflare/workers-types` 版本 |
| `workers_types_git_head` | 该 types 包对应的 upstream git revision |
| `workers_types_package_sha256` | types 包 digest |
| `workers_types_index_sha256` | 固定 stable `index.d.ts` 字节 SHA-256 |
| `workers_types_ast_sha256` | 固定 stable 声明的 canonical AST SHA-256 |

不要改二进制旁的 workerd、不要 PATH 搜索、不要另下一份 runtime。digest 对不上就停。

## products

按稳定产品名索引。每条包含：

| 字段 | 含义 |
| --- | --- |
| `status` | `supported` / `supported_with_deviation` / `blocked` / `unsupported` |
| `kind` | `target`（上游 AST inventory）、`platform`（本平台产品）、`non_target`（明确非目标） |
| `capability_version` | 完整支持时的静态 facade 版本；`blocked` / `unsupported` 时省略 |
| `members` | 目标产品的逐成员/overload 记录；每条有 stable id、symbol、member、kind、overload、readonly/optional/static、signature 与 `signature_sha256`、状态和证据 case |
| `deviations` | 已登记的单机拓扑或 stock-runtime 容量偏差 ID |

`supported`：该产品全部目标成员都有 compile 和真实 runtime 证据。`supported_with_deviation`：API 完整，仅存在已登记的单机拓扑差异。`blocked`：属于目标，但实现或证据未完成；不得声称兼容。`unsupported`：第 5 节明确非目标产品。目标缺口不能标成 `unsupported`。没有精确证据的目标成员保持 `blocked`，不能用产品 smoke test 或类型存在推导支持。

当前 2,097 个目标成员没有 `blocked` 项。Workers、KV、R2、D1、Durable Objects、Queues、Cron、Workflows 和 Cache API 因已登记的单机差异为 `supported_with_deviation`；Alarms、Version Metadata 和 WebSocket hibernation 为 `supported`。D1 覆盖 database/session/prepared-statement/result/meta、错误与 bind 转换、原子 batch、opaque bookmark 及当前 hosted 非 alpha `dump()` 拒绝。raw TCP general outbound 使用唯一 public Network 和 stock workerd 的 `cloudflare:sockets`/Node socket；命名 Service/DO 的 `Fetcher.connect()` 走显式 capability tunnel。`deployments`、`static_assets`、`service_bindings`、`workers_cache` 和本地有界 Images 是平台产品；Images 不宣称完整托管 Cloudflare Images。`ai`、`vectorize` 等明确非目标产品是 `unsupported`。

## limits

配置里冻结的产品数值上限，**不含密钥**。精确数字以当前 `capabilities --json` 为准，不要把本页或默认 TOML 抄成运行中的配额。pinned stock OSS workerd 的 standalone `LimitEnforcer` 不执行 Cloudflare 托管环境的 request-scoped CPU、subrequest 或 simultaneous-connection quota；该差异见 `OC-WKR-LIMIT-001`，不能从其它 `limits` 字段推导这些配额已生效。

## 已登记偏差

这些 ID 会出现在对应产品的 `deviations` 里。含义以登记为准；没有 ID 不等于 Cloudflare 其它行为可用。

### OC-WKR-TCP-001

tenant 的 general outbound `fetch()`、`cloudflare:sockets.connect()` 和 `node:net` 共享唯一的 stock-workerd `Network(allow = ["public"])`；命名 Service/DO 的 `Fetcher.connect()` 走声明式 capability tunnel，不是第二个通用 outbound。open-compute 不复制 Cloudflare 自有 IP 段封禁、Worker self-connect/TCP-loop detector 或默认 SMTP 25 封禁。runtime-source、binding-backend 和 workerd 内部 listener 强制 loopback；control/data listener 默认 loopback，但 operator 可以显式暴露，因此不能宣称 public Network 会按“平台所有权”额外拒绝公开地址。operator 负责公开入口和额外公网/SMTP egress policy。

### OC-WKR-LIMIT-001

pinned stock OSS workerd standalone `LimitEnforcer` 不执行 subrequest/CPU 限制。open-compute 不声称 Cloudflare request-scoped CPU、subrequest 或 simultaneous-connection quota；这不放宽 public-address 安全边界、产品专有限额、handle 清理和进程监督。

### OC-KV-001

KV 是单节点 SQLite 权威存储，不声称 Cloudflare 全球复制或传播时延。

### OC-R2-001

R2 object bytes 由配置的 S3-compatible provider 持有。不声称 Cloudflare 全球 placement 或 replication。

### OC-D1-001

D1 是单个本地主 SQLite authority，不声称 read replica、region routing、hosted `served_by` 身份、
region/colo metadata 或 Cloudflare 计费计数；opaque bookmark 保证同一数据库的本地顺序可见性，
`rows_read`/`rows_written` 是稳定的本地 SQLite 执行计数。

### OC-DO-001

Durable Objects 落在本地这一个 workerd 进程上。location hint、jurisdiction 和全球迁移没有地理调度效果。

### OC-QUEUE-001

Queue producer 和 push consumer 的耐久性来自单节点 `scheduler.sqlite`，不是 Cloudflare 全球复制。投递是 at-least-once，没有全球 FIFO。未知的 native dispatch 会保留 lease，不消耗租户重试预算，所以后续投递可能重复同一 attempt number。

### OC-CRON-001

Cron 只有 UTC、五个字段，以及已文档化的本机 Quartz-like 扩展。恢复时最多投影 misfire grace 内最新的一个 slot，不会重放完整停机历史。已知失败走配置里的有界本机重试，除非调用了 `noRetry()`。

### OC-WORKFLOW-001

Workflow 在本地 SQLite authority 上执行。callback 在结果提交前是 at-least-once；replay 会跳过已耐久完成的 callback；外部产品副作用不会随 Workflow snapshot 回滚。不声称跨地域执行、全球 placement 或 Cloudflare dashboard/observability。

### OC-CACHE-001

Workers Cache 和 Cache API 是单节点本机权威。自动缓存需要显式 `s-maxage` 或 `max-age`。不支持启发式 TTL、全球复制/purge 传播、tiered cache、Cache Rules、Cache Deception Armor，以及依赖套餐的行为。

### OC-CACHE-002

运维配置的默认值是每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值由 `platformd capabilities --json` 给出。

### OC-IMAGES-001

Images 是有界的本机光栅变换 binding，不是托管的 Cloudflare Images。托管投递/上传/签名、URL transform、视频、AI upscale 和 Cloudflare 产品配额不在范围内。

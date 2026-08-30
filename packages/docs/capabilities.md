# 能力与限制

以这台机器上的二进制为准，不要用产品名字推导 Cloudflare 全量行为。未出现在 `capabilities --json` 里的 Cloudflare 功能视为不支持。本页不是一份完整 Cloudflare 矩阵。

```sh
platformd --config /etc/open-compute/config.toml capabilities --json
```

`--config` 对 `capabilities` 是可选的。省略时，`limits` 来自内嵌默认配置；给出绝对路径配置时，`limits` 反映该文件。JSON 顶层字段如下。

## schema_version

固定为 `1`。其它值不要当这份契约来读。

## release

精确的发行身份，不是营销版本号。包含 `platform_version`、`git_revision`、`rust_msrv`、`workerd_version`、`workerd_lock_sha256`、`runtime_assets_sha256`、`facade_capability_version`，以及 control / scheduler / KV / D1 的 schema 版本、`snapshot_format_version` 和 `compatibility_policy_sha256`。恢复和替换二进制时，拿这里的身份去对快照与 schema，而不是看文件名。

## runtime

pinned workerd 的兼容策略：

| 字段 | 含义 |
| --- | --- |
| `compatibility_date_min` / `compatibility_date_max` | Worker compatibility date 闭区间 |
| `allowed_flags` | 允许的 compatibility flags |
| `denied_flags` | 明确拒绝的 flags |
| `workerd_lock_sha256` | 正式 runtime lock 字节的 SHA-256 |

不要改二进制旁的 workerd、不要 PATH 搜索、不要另下一份 runtime。digest 对不上就停。

## products

按稳定产品名索引。每条包含：

| 字段 | 含义 |
| --- | --- |
| `status` | `supported` / `unsupported` / `conditional` |
| `capability_version` | `supported` 时有静态 facade 版本；`unsupported` / `conditional` 时省略 |
| `methods` | 支持的方法名，规范顺序 |
| `deviations` | 已登记的偏差 ID |
| `basic_websocket` | Durable Objects 可选；基础 WebSocket 状态 |
| `hibernatable_websocket` | Durable Objects 可选；休眠 WebSocket 状态 |

`supported`：生产行为及其 Gate 已实现。`unsupported`：故意缺席。`conditional`：pinned runtime 的硬 Gate 还没有稳定 Go。未列出的产品或 API 不要当成支持。

当前登记的产品名包括 `workers`、`kv`、`r2`、`d1`、`durable_objects`、`alarms`、`queues`、`cron`、`workflows`、`workers_cache`、`cache_api`、`images`、`version_metadata`、`websocket_hibernation`。Durable Objects 另报 `basic_websocket`（supported）和 `hibernatable_websocket`（unsupported）。`websocket_hibernation` 产品本身是 `unsupported`。

## limits

配置里冻结的数值上限，**不含密钥**。精确数字以当前 `capabilities --json` 为准，不要把本页或默认 TOML 抄成运行中的配额。

## 已登记偏差

这些 ID 会出现在对应产品的 `deviations` 里。含义以登记为准；没有 ID 不等于 Cloudflare 其它行为可用。

### OC-KV-001

KV 是单节点 SQLite 权威存储，不声称 Cloudflare 全球复制或传播时延。

### OC-R2-001

R2 绑在配置的 S3 authority 上。整机快照会记录 bucket 身份，但不提供 R2 的 point-in-time recovery。

### OC-D1-001

D1 的 session 约束和 bookmark 复制未实现；`withSession()` 只暴露已文档化的本机行为。

### OC-DO-001

Durable Objects 落在本地这一个 workerd 进程上；placement hints 和全球迁移不支持。

### OC-WS-001

基础 Durable Object WebSocket 支持。原生 hibernatable WebSocket 保持关闭，直到 pinned stock-workerd 硬 Gate 给出完整 Go。

### OC-QUEUE-001

Queue producer 和 push consumer 的耐久性来自单节点 `scheduler.sqlite`，不是 Cloudflare 全球复制。投递是 at-least-once，没有严格 FIFO。未知的 native dispatch 会保留 lease，不消耗租户重试预算，所以后续投递可能重复同一 attempt number。所有已支持的 compatibility date 默认 JSON；`v8`、metadata、pull consumer、每 Queue 多个 consumer、资源级 PITR、Cloudflare 套餐配额都不支持。Durable Object 内写 Queue 失败关闭，错误为 `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED`：service-facade 传输继承不了 stock workerd 的原生 Durable Object output gate。

### OC-CRON-001

Cron 只有 UTC、五个字段，以及已文档化的本机 Quartz-like 扩展。恢复时最多投影 misfire grace 内最新的一个 slot，不会重放完整停机历史。已知失败走配置里的有界本机重试，除非调用了 `noRetry()`。

### OC-WORKFLOW-001

Workflow 用本机 SQLite 权威存储和有界 canonical JSON，不是完整 structured clone。当前模型支持重试、attempt timeout、durable sleep 与事件等待、lifecycle modifiers、冻结 retention，以及有界同步 `step.do` 批次。不支持并行 wait、任意 Promise 图、批量创建 instance、动态 retry 函数、rollback hooks、从某一步 restart、完整 structured clone、外部副作用 exactly-once。callback 在结果提交前是 at-least-once；replay 会跳过已耐久完成的 callback；外部产品副作用不会随 Workflow snapshot 回滚。

### OC-WORKFLOW-002

Durable Object 上的 Workflow 变更（`create`、`sendEvent`、`pause`、`resume`、`terminate`、`restart`）失败关闭，错误为 `WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED`：独立的 pinned-workerd 探测无法证明这条 service-facade 传输继承了原生 Durable Object output gate。只读的 `get` 和 `status` 仍可用。反过来，Workflow 调 DO 仍走现有的活跃 Worker 部署检查；已退役部署得到 `DO_DEPLOYMENT_STALE`。

### OC-CACHE-001

Workers Cache 和 Cache API 是单节点本机权威。自动缓存需要显式 `s-maxage` 或 `max-age`。不支持启发式 TTL、全球复制/purge 传播、tiered cache、Cache Rules、Cache Deception Armor，以及依赖套餐的行为。

### OC-CACHE-002

运维配置的默认值是每个缓存对象 16 MiB、每个 Worker 1 GiB 逻辑 body 字节，不是 Cloudflare 更大的产品配额。运行中的精确值由 `platformd capabilities --json` 给出。

### OC-IMAGES-001

Images 是有界的本机光栅变换 binding，不是托管的 Cloudflare Images。Day1 输入是 JPEG/PNG/WebP；输出是 JPEG/PNG/WebP/AVIF。不支持动画输入、任意 ICC 保留、SVG、托管投递/上传/签名、URL transform、视频、AI upscale，以及 `fetch(..., {cf:{image}})`。

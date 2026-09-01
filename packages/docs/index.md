# open-compute

在单节点上构建和运行 Workers 应用的无服务器平台。open-compute 运行已声明的 Cloudflare Workers 编程模型（`ocd` + 固定版本的 `workerd`）。不提供全球边缘、计费或 Cloudflare dashboard。

用 `oc` 部署模块 Worker；用 `ocd` 作为平台进程运行。项目文件是 `open-compute.json`。

[开始](/get-started) · [产品目录](/directory)

## Compute

- [Workers](/workers/) — 模块 Worker，由本机 `workerd` 执行
- [Durable Objects](/durable-objects/) — 带强一致存储的有状态计算
- [Workflows](/workflows/) — 可重放的多步应用
- [Queues](/queues/) — 至少一次投递的消息

## Storage

- [KV](/kv/) — 低延迟键值
- [D1](/d1/) — SQL
- [R2](/r2/) — 对象存储（字节在你配置的 S3）

## Media

- [Cache](/workers/cache/) — Workers Cache 与 Cache API
- [Images](/images/) — 有界的本机光栅变换

## Platform

- [平台](/platform/) — 兼容性、限制与行为差异
- [Limits](/platform/limits) — 运行中二进制的 `capabilities.limits`
- [Compatibility](/platform/compatibility) — 产品、Worker API、单节点拓扑
- [行为差异](/platform/deviations) — 单节点拓扑与运行时行为

## Operate

安装 `ocd`、编写配置、作为服务运行，以及故障手册：[ocd](/ocd/)。

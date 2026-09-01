# open-compute

一台机器上的 Workers 平台：声明的编程模型，本地运行。

兼容的是已声明的 Workers 编程模型，不是全球边缘、计费或 Cloudflare dashboard。开了什么、故意做成什么样，以这台机器上的 `ocd capabilities --json` 为准。

[开始](/get-started) · [产品目录](/directory)

## Compute

- [Workers](/workers/) — 模块 Worker，本机 `workerd` 执行
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

- [平台](/platform/) — 契约入口
- [Limits](/platform/limits) — 运行中二进制的 `capabilities.limits`
- [Compatibility](/platform/compatibility) — 契约、状态语义、成员库存
- [Deviations](/platform/deviations) — 已登记的单机差异（`OC-*`）

## Operate

安装 `ocd`、写配置、把它当服务跑，以及故障手册：[ocd](/ocd/)。

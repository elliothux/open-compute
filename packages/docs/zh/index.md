# open-compute

单机 Workers 平台。`ocd` 启动锁定版本的 `workerd`，Worker API 与 Cloudflare 文档一致。不提供全球边缘网络、计费或 Cloudflare 控制台。

使用 `oc` 部署 Worker，使用 `ocd` 运行平台。项目配置为 `wrangler.jsonc`。

[开始](/zh/get-started) · [产品目录](/zh/directory)

## 计算

- [Workers](/zh/workers/) — 在本机 `workerd` 中运行模块 Worker
- [Durable Objects](/zh/durable-objects/) — 有状态对象，存储强一致
- [Workflows](/zh/workflows/) — 可从中断处恢复的多步工作流
- [Queues](/zh/queues/) — Worker 间消息队列（at-least-once）

## 存储

- [KV](/zh/kv/) — 键值存储
- [D1](/zh/d1/) — SQL
- [R2](/zh/r2/) — 对象存储（数据位于配置的 S3）

## 媒体

- [Cache](/zh/workers/cache/) — Workers Cache 与 Cache API
- [Images](/zh/images/) — 本地图像变换（受尺寸与并发限制）

## 平台

- [平台](/zh/platform/) — 兼容性、限制与行为差异
- [限制](/zh/platform/limits) — 以运行中的 `ocd capabilities --json` 为准
- [兼容性](/zh/platform/compatibility) — 产品、Worker API 与数据位置
- [行为差异](/zh/platform/deviations) — 相对 Cloudflare 托管环境的差异

## 运维

安装 `ocd`、编写配置、作为服务运行，以及故障手册：[ocd](/zh/ocd/)。

---
title: "兼容性"
description: "open-compute 当前 Cloudflare Workers 兼容范围与明确的单机差异。"
---

open-compute 在单机实现声明的 Cloudflare Workers 编程模型。Worker 代码与受支持 binding 遵循 Cloudflare 公开 API；placement、replication、quota 和 management 由选中的本机 `ocd` authority 拥有。

查看运行中 release 的精确合同：

```sh
ocd capabilities --json
```

指定 `--config <absolute-path>` 时，配置 limit 来自该文件；省略时使用内嵌默认值。

## 支持的产品组

- Module Workers、Versions、Deployments、Static Assets、Service Bindings、Version Metadata、WebSockets 与文档列出的 runtime APIs
- KV、D1、R2、Durable Objects、Alarms、Queues、Cron、Workflows、Workers Cache 与 Cache API
- Images、Vectorize、AI Search、Markdown Conversion、Workers Logs/realtime tail 与 Artifacts
- Cloudflare-compatible `/client/v4` 管理 API、认证 Wrangler 工作流和 operator Dashboard

多数产品状态为 `supported_with_deviation`，因为它们使用单机 local authority，而不是 Cloudflare 托管全球拓扑。Vectorize 使用确定性的精确搜索。AI Search 与 Markdown Conversion 使用 operator-configured provider，不代表提供完整 Workers AI inference。

## Runtime 与项目合同

正式 release 内嵌由 formal runtime lock 选择并校验 checksum 的 `elliothux/workerd` fork，生产启动保持离线。项目使用标准 `wrangler.jsonc` 和项目内 Wrangler。只有当前 runtime contract 支持的 compatibility date 与 flag 才能通过 admission。

Dynamic Worker Loader 由原生 runtime 提供有界 surface，但仍缺少文档所述 CPU、memory 和 subrequest enforcement，因此不代表完整 Workers for Platforms 产品。

参见[产品](/docs/zh/products/)、[行为差异](/docs/zh/platform/deviations/)、[限制](/docs/zh/platform/limits/)、[未提供能力](/docs/zh/platform/unsupported/)和[生成的 Worker API 索引](/docs/zh/platform/reference/api/)。

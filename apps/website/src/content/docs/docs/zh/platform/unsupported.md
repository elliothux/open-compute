---
title: "未提供能力"
description: "open-compute 当前未提供的 Cloudflare 平台能力。"
---

upstream type 或 Wrangler field 的存在不代表 open-compute 会注入对应 capability。不支持的配置会在 admission 阶段失败，不会创建 placeholder binding。

## 当前排除项

- Browser Run 与 browser rendering
- Containers 与 Cloudchamber
- Hyperdrive
- Analytics Engine
- 完整 Workers for Platforms 与 dispatch namespace
- 通用 Workers AI model inference、model catalog 与 AutoRAG
- Pipelines
- Rate Limiting
- mTLS certificates
- Tail Workers、distributed trace export 与 Logpush

Dynamic Worker Loader 已有有界原生 surface，但完整产品仍被标准 CPU、memory 和 subrequest limit enforcement 阻塞。AI Search 和 Markdown Conversion 不会开放其它 Workers AI 方法。

Artifacts 是当前受支持产品，见 [Artifacts](/docs/zh/artifacts/)。Browser Run 与 Containers 已有设计工作，但还不是可部署 capability。

参见[产品](/docs/zh/products/)与[兼容性](/docs/zh/platform/compatibility/)。

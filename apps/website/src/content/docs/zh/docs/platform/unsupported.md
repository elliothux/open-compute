---
title: "未提供能力"
description: "open-compute 当前未提供的 Cloudflare 平台能力。"
---

upstream type 或 cf field 的存在不代表 open-compute 会注入对应 capability。不支持的配置会在 admission 阶段失败，不会创建 placeholder binding。

## 当前排除项

- Containers 与 Cloudchamber
- Hyperdrive
- Analytics Engine
- 完整 Workers for Platforms 与 dispatch namespace
- 通用 Workers AI model inference、model catalog 与 AutoRAG
- Pipelines
- Rate Limiting
- mTLS certificates
- Tail Workers、distributed trace export 与 Logpush

Dynamic Worker Loader 已支持文档所列的本机 limit 与偏差。完整 Workers for Platforms、dispatch namespace，以及实验性的 `allowExperimental` 与 `streamingTails` 控制仍不在支持范围内。AI Search 和 Markdown Conversion 不会开放其它 Workers AI 方法。

Artifacts 是当前受支持产品，见 [Artifacts](/zh/docs/artifacts/)。Browser Run 在显式配置 backend 后可用，限制见 [Browser Run](/zh/docs/browser-run/)。Containers 仍处于规划阶段，尚不可部署。

参见[产品](/zh/docs/products/)与[兼容性](/zh/docs/platform/compatibility/)。

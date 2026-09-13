---
title: "产品"
description: "open-compute 支持的 Workers、存储、计算、媒体、AI 与 Artifacts 能力。"
---

产品状态来自当前版本的 capability contract 和兼容证据。运行中二进制以 `ocd capabilities --json` 为准。

## 可用，但存在明确的单机差异

| 领域             | 产品                                                                                                                              |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Runtime 与交付   | [Workers](/zh/docs/workers/)、Versions 与 Deployments、Static Assets、Service Bindings、Version Metadata                          |
| 存储             | [KV](/zh/docs/kv/)、[D1](/zh/docs/d1/)、[R2](/zh/docs/r2/)、Workers Cache 与 Cache API                                            |
| 有状态与调度计算 | [Durable Objects](/zh/docs/durable-objects/)、Alarms、[Queues](/zh/docs/queues/)、Cron Triggers、[Workflows](/zh/docs/workflows/) |
| 媒体与检索       | [Images](/zh/docs/images/)、[Vectorize](/zh/docs/vectorize/)、[AI Search](/zh/docs/ai-search/)、Markdown Conversion               |
| 源码 Artifacts   | [Artifacts](/zh/docs/artifacts/)                                                                                                  |

Vectorize 在单机执行确定性的精确搜索。AI Search 和 Markdown Conversion 使用 operator-configured provider，不代表支持通用 Workers AI model inference。

## 未提供

Browser Run、Containers、Hyperdrive、Analytics Engine、完整 Workers for Platforms、通用 Workers AI inference、Pipelines、Rate Limiting 和 mTLS certificates 不是当前产品。需要未提供能力的配置会 fail closed。

当前边界参见[兼容性](/zh/docs/platform/compatibility/)、[行为差异](/zh/docs/platform/deviations/)、[限制](/zh/docs/platform/limits/)和[未提供能力](/zh/docs/platform/unsupported/)。

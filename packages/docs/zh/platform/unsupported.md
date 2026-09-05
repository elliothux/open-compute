# 不支持

open-compute 在部署 authority 边界拒绝下列 Cloudflare 开发者平台产品。不要仅因 upstream types 中存在同名符号，就将其视为已注入的 binding。

**已提供、但有边界** 的产品见对应文档：[Vectorize](/zh/vectorize/)、[AI Search](/zh/ai-search/)（含经 `env.AI` 的 Markdown Conversion）。**不提供**完整 Workers AI 模型推理。

| 名称 / 配置字段 | Cloudflare 产品 | 状态 |
| --- | --- | --- |
| `browser` / `browser_rendering` | [Browser Run](https://developers.cloudflare.com/browser-rendering/)（原 Browser Rendering） | 尚未 — 仅设计（[P12](https://github.com/elliothux/open-compute/blob/main/docs/p12-browser-run.md)） |
| `artifacts` | Artifacts | 尚未 — 仅设计（[P11](https://github.com/elliothux/open-compute/blob/main/docs/p11-cloudflare-artifacts.md)） |
| `containers` / `cloudchamber` | Containers | 尚未 |
| `hyperdrive` | Hyperdrive | 尚未 |
| `analytics_engine` / `analytics_engine_datasets` | Analytics Engine | 尚未 |
| `workers_for_platforms` / `dispatch_namespaces` | Workers for Platforms | 尚未 |
| `worker_loaders` | Dynamic Workers | 尚未 |
| `pipelines` | Pipelines | 尚未 |
| `rate_limiting` / `ratelimits` | Rate Limiting | 尚未 |
| `mtls` / `mtls_certificates` | mTLS certificates | 尚未 |
| Workers AI `run()` / `models()` / AutoRAG / 其它推理 | 完整 Workers AI 模型推理 | 尚未 — 已声明的 `env.AI` 子集见 [AI Search](/zh/ai-search/) |

纯边缘/拓扑差异（Anycast、全球复制、托管 fleet 配额）不作为“缺产品”列出，见[行为差异](/zh/platform/deviations)。

已提供的产品见[产品目录](/zh/directory)和[兼容性](/zh/platform/compatibility)。

# 不支持

open-compute 在部署边界拒绝下列 Cloudflare 开发者平台产品。upstream TypeScript types 里出现同名符号，**不**表示已注入对应 binding。

**部分支持（有边界）：** [Vectorize](/zh/vectorize/)、[AI Search](/zh/ai-search/)，以及经 `env.AI` 的 Markdown Conversion。

| 配置 / binding | Cloudflare 产品 | 状态 |
| --- | --- | --- |
| `browser` / `browser_rendering` | [Browser Run](https://developers.cloudflare.com/browser-rendering/)（原 Browser Rendering） | 规划中 |
| `artifacts` | [Artifacts](https://developers.cloudflare.com/artifacts/) | 规划中 |
| `ai` 模型推理（`run` / 目录 / AutoRAG） | [Workers AI](https://developers.cloudflare.com/workers-ai/) | 尚未支持 — Markdown Conversion 与 AI Search 仅在各自场景使用 `env.AI` |
| `containers` / `cloudchamber` | Containers | 尚未支持 |
| `hyperdrive` | Hyperdrive | 尚未支持 |
| `analytics_engine` / `analytics_engine_datasets` | Analytics Engine | 尚未支持 |
| `workers_for_platforms` / `dispatch_namespaces` | Workers for Platforms | 尚未支持 |
| `worker_loaders` | Dynamic Workers | 尚未支持 |
| `pipelines` | Pipelines | 尚未支持 |
| `rate_limiting` / `ratelimits` | Rate Limiting | 尚未支持 |
| `mtls` / `mtls_certificates` | mTLS certificates | 尚未支持 |
| Tail Workers / traces export / Logpush | Workers 可观测性扩展 | 尚未支持 |

纯边缘差异（Anycast、全球复制、托管 fleet 配额）不作为“缺产品”列出，见[行为差异](/zh/platform/deviations)。

已提供产品：[产品目录](/zh/directory) · [兼容性](/zh/platform/compatibility)。

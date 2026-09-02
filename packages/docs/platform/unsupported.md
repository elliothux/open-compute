# 不支持

open-compute 不提供下列 Cloudflare 产品。部署时会在边界拒绝它们。不要仅因 upstream types 中存在同名符号，就将其视为已注入的 binding。

| 名称 | Cloudflare 产品 |
| --- | --- |
| `analytics_engine` | Analytics Engine |
| `ai` | Workers AI |
| `browser_rendering` | Browser Rendering |
| `vectorize` | Vectorize |
| `hyperdrive` | Hyperdrive |
| `mtls` | mTLS certificates |
| `rate_limiting` | Rate Limiting |
| `workers_for_platforms` | Workers for Platforms |

已提供的产品见[产品目录](/directory)和[兼容性](/platform/compatibility)。

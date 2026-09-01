# 不支持

下列产品在契约里是 `non_target` / `unsupported`。它们不是本平台产品，不要写成产品页，也不要因为 upstream types 里有同名符号就当成已注入。

| 稳定名 | Cloudflare 产品 |
| --- | --- |
| `analytics_engine` | Analytics Engine |
| `ai` | Workers AI |
| `browser_rendering` | Browser Rendering |
| `vectorize` | Vectorize |
| `hyperdrive` | Hyperdrive |
| `mtls` | mTLS certificates |
| `rate_limiting` | Rate Limiting |
| `workers_for_platforms` | Workers for Platforms |

部署 authority 会在边界拒绝它们。支持面见[目录](/directory)和[兼容性](/platform/compatibility)。

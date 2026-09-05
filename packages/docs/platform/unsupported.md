# Unsupported

open-compute rejects the Cloudflare developer-platform products below at the deployment authority boundary. Do not treat a name in upstream types as an injected binding.

Partial surfaces that **are** provided (with documented limits) live on their product pages: [Vectorize](/vectorize/), [AI Search](/ai-search/) (includes Markdown Conversion via `env.AI`). Full Workers AI model inference is **not** provided.

| Name / config field | Cloudflare product | Status |
| --- | --- | --- |
| `browser` / `browser_rendering` | [Browser Run](https://developers.cloudflare.com/browser-rendering/) (formerly Browser Rendering) | Not yet — design only ([P12](https://github.com/elliothux/open-compute/blob/main/docs/p12-browser-run.md)) |
| `artifacts` | Artifacts | Not yet — design only ([P11](https://github.com/elliothux/open-compute/blob/main/docs/p11-cloudflare-artifacts.md)) |
| `containers` / `cloudchamber` | Containers | Not yet |
| `hyperdrive` | Hyperdrive | Not yet |
| `analytics_engine` / `analytics_engine_datasets` | Analytics Engine | Not yet |
| `workers_for_platforms` / `dispatch_namespaces` | Workers for Platforms | Not yet |
| `worker_loaders` | Dynamic Workers | Not yet |
| `pipelines` | Pipelines | Not yet |
| `rate_limiting` / `ratelimits` | Rate Limiting | Not yet |
| `mtls` / `mtls_certificates` | mTLS certificates | Not yet |
| Workers AI `run()` / `models()` / AutoRAG / unrelated inference | Full Workers AI model inference | Not yet — see [AI Search](/ai-search/) for the declared `env.AI` subset |

Pure edge/topology gaps (Anycast, global replication, hosted fleet quotas) are not listed here as missing products; see [Behavior differences](/platform/deviations).

The provided surface is the [Directory](/directory) and [Compatibility](/platform/compatibility).

# Unsupported

open-compute rejects the Cloudflare developer-platform products below at the deployment boundary. A name in upstream TypeScript types does **not** mean the binding is injected.

**Partial (available with limits):** [Vectorize](/vectorize/), [AI Search](/ai-search/), and Markdown Conversion via `env.AI`.

| Config / binding | Cloudflare product | Status |
| --- | --- | --- |
| `browser` / `browser_rendering` | [Browser Run](https://developers.cloudflare.com/browser-rendering/) (formerly Browser Rendering) | Planning |
| `artifacts` | [Artifacts](https://developers.cloudflare.com/artifacts/) | Planning |
| `ai` model inference (`run` / catalog / AutoRAG) | [Workers AI](https://developers.cloudflare.com/workers-ai/) | Not yet — Markdown Conversion and AI Search use `env.AI` only for their own surfaces |
| `containers` / `cloudchamber` | Containers | Not yet |
| `hyperdrive` | Hyperdrive | Not yet |
| `analytics_engine` / `analytics_engine_datasets` | Analytics Engine | Not yet |
| `workers_for_platforms` / `dispatch_namespaces` | Workers for Platforms | Not yet |
| `worker_loaders` | Dynamic Workers | Not yet |
| `pipelines` | Pipelines | Not yet |
| `rate_limiting` / `ratelimits` | Rate Limiting | Not yet |
| `mtls` / `mtls_certificates` | mTLS certificates | Not yet |
| Tail Workers / traces export / Logpush | Workers observability extras | Not yet |

Edge-only gaps (Anycast, global replication, hosted fleet quotas) are not listed as missing products — see [Behavior differences](/platform/deviations).

Provided products: [Directory](/directory) · [Compatibility](/platform/compatibility).

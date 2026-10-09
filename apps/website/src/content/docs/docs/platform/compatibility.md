---
title: "Compatibility"
description: "Current Cloudflare Workers compatibility and the intentional single-node differences in open-compute."
---

open-compute implements the declared Cloudflare Workers programming model on one node. Worker code and supported bindings follow Cloudflare's public APIs; placement, replication, quotas, and management remain local to the selected instance authority.

Inspect the exact running release:

```sh
ocd capabilities --json
```

With `--config <absolute-path>`, configured limits come from that file. Without it, the command reports the embedded defaults.

## Supported product families

- Module Workers, Versions, Deployments, Static Assets, Service Bindings, Version Metadata, WebSockets, and the documented runtime APIs
- KV, D1, R2, Durable Objects, Alarms, Queues, Cron, Workflows, Workers Cache, and Cache API
- Images, Vectorize, AI Search, Markdown Conversion, Workers Logs/realtime tail, and Artifacts
- Cloudflare-compatible `/client/v4` management APIs, the documented cf workflow, and the operator Dashboard

Most products are reported as `supported_with_deviation` because they use a single local authority instead of Cloudflare's hosted global topology. Vectorize uses deterministic exact search. AI Search and Markdown Conversion use operator-configured providers; full Workers AI inference is not implied. The separately typed `open-compute:manual` AI Search source is a namespaced open-compute API superset and is excluded from the Cloudflare stable-member inventory.

Browser Run supports configured CDP and managed backends with explicit client and download limits; see [Browser Run](/docs/browser-run/).

## Runtime and project contract

The release embeds a checksum-verified `elliothux/workerd` fork selected by the formal runtime lock. Production startup remains offline. Projects use standard `cloudflare.config.ts` and project-local cf. The exact binary validates each Version's unchanged compatibility date and flags; its reflected catalog is exposed for discovery and is not a second admission policy.

Dynamic Worker Loader support is native and enforces the documented local CPU, memory, subrequest, startup, and simultaneous-connection ceilings. The two experimental trust/tail members remain blocked, so this is not a claim of the complete Workers for Platforms product.

See [Products](/docs/products/), [Logs and live tail](/docs/workers/observability/), [Behavior differences](/docs/platform/deviations/), [Limits](/docs/platform/limits/), [Not available](/docs/platform/unsupported/), and the [API and product index](/docs/platform/reference/api/).

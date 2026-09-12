---
title: "Products"
description: "Supported open-compute Workers, storage, compute, media, AI, and artifact capabilities."
---

Product status comes from the current release capability contract and its compatibility evidence. `ocd capabilities --json` is authoritative for the running binary.

## Available with documented single-node differences

| Area                           | Products                                                                                                                 |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| Runtime and delivery           | [Workers](/docs/workers/), Versions and Deployments, Static Assets, Service Bindings, Version Metadata                   |
| Storage                        | [KV](/docs/kv/), [D1](/docs/d1/), [R2](/docs/r2/), Workers Cache and Cache API                                           |
| Stateful and scheduled compute | [Durable Objects](/docs/durable-objects/), Alarms, [Queues](/docs/queues/), Cron Triggers, [Workflows](/docs/workflows/) |
| Media and retrieval            | [Images](/docs/images/), [Vectorize](/docs/vectorize/), [AI Search](/docs/ai-search/), Markdown Conversion               |
| Source artifacts               | [Artifacts](/docs/artifacts/)                                                                                            |

Vectorize uses deterministic exact search on one node. AI Search and Markdown Conversion use operator-configured providers and do not imply general Workers AI model inference.

## Not available

Browser Run, Containers, Hyperdrive, Analytics Engine, full Workers for Platforms, general Workers AI inference, Pipelines, Rate Limiting, and mTLS certificates are not current products. Configuration that requires an unavailable capability fails closed.

See [Compatibility](/docs/platform/compatibility/), [Behavior differences](/docs/platform/deviations/), [Limits](/docs/platform/limits/), and [Not available](/docs/platform/unsupported/) for the current boundaries.

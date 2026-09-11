---
title: "open-compute"
---

A serverless platform for building Workers applications on a single node. open-compute runs the declared Cloudflare Workers programming model (`ocd` + pinned `workerd`). It does not provide a global edge, billing, or the Cloudflare dashboard.

Deploy module Workers with `oc`. Run the platform as a service with `ocd`. The project file is `wrangler.jsonc`.

[Get started](/docs/get-started) · [Directory](/docs/directory)

## Compute

- [Workers](/docs/workers/) — Module Workers, executed by local `workerd`
- [Durable Objects](/docs/durable-objects/) — Stateful compute with strongly consistent storage
- [Workflows](/docs/workflows/) — Replayable multi-step applications
- [Queues](/docs/queues/) — At-least-once message delivery

## Storage

- [KV](/docs/kv/) — Low-latency key-value storage
- [D1](/docs/d1/) — SQL
- [R2](/docs/r2/) — Object storage (bytes live on the selected Local or S3 authority)

## Media

- [Cache](/docs/workers/cache/) — Workers Cache and the Cache API
- [Images](/docs/images/) — Bounded local raster transforms

## Platform

- [Platform](/docs/platform/) — Compatibility, limits, and behavior differences
- [Limits](/docs/platform/limits) — `capabilities.limits` from the running binary
- [Compatibility](/docs/platform/compatibility) — Products, Worker APIs, single-node topology
- [Behavior differences](/docs/platform/deviations) — Single-node topology and runtime behavior

## Operate

Install `ocd`, write config, run it as a service, and the incident handbook: [ocd](/docs/ocd/).

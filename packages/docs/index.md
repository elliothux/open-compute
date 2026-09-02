# open-compute

A serverless platform for building Workers applications on a single node. open-compute runs the declared Cloudflare Workers programming model (`ocd` + pinned `workerd`). It does not provide a global edge, billing, or the Cloudflare dashboard.

Deploy module Workers with `oc`. Run the platform as a service with `ocd`. The project file is `open-compute.json`.

[Get started](/get-started) · [Directory](/directory)

## Compute

- [Workers](/workers/) — Module Workers, executed by local `workerd`
- [Durable Objects](/durable-objects/) — Stateful compute with strongly consistent storage
- [Workflows](/workflows/) — Replayable multi-step applications
- [Queues](/queues/) — At-least-once message delivery

## Storage

- [KV](/kv/) — Low-latency key-value storage
- [D1](/d1/) — SQL
- [R2](/r2/) — Object storage (bytes live on the S3 you configured)

## Media

- [Cache](/workers/cache/) — Workers Cache and the Cache API
- [Images](/images/) — Bounded local raster transforms

## Platform

- [Platform](/platform/) — Compatibility, limits, and behavior differences
- [Limits](/platform/limits) — `capabilities.limits` from the running binary
- [Compatibility](/platform/compatibility) — Products, Worker APIs, single-node topology
- [Behavior differences](/platform/deviations) — Single-node topology and runtime behavior

## Operate

Install `ocd`, write config, run it as a service, and the incident handbook: [ocd](/ocd/).

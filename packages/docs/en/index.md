# open-compute

A Workers platform on one machine: the declared programming model, run locally.

Compatibility is the declared Workers programming model, not a global edge, billing, or the Cloudflare dashboard. What is enabled, and which differences are intentional, is `ocd capabilities --json` on that machine.

[Get started](/en/get-started) · [Directory](/en/directory)

## Compute

- [Workers](/en/workers/) — Module Workers, executed by local `workerd`
- [Durable Objects](/en/durable-objects/) — Stateful compute with strongly consistent storage
- [Workflows](/en/workflows/) — Replayable multi-step applications
- [Queues](/en/queues/) — At-least-once message delivery

## Storage

- [KV](/en/kv/) — Low-latency key-value storage
- [D1](/en/d1/) — SQL
- [R2](/en/r2/) — Object storage (bytes live on the S3 you configured)

## Media

- [Cache](/en/workers/cache/) — Workers Cache and the Cache API
- [Images](/en/images/) — Bounded local raster transforms

## Platform

- [Platform](/en/platform/) — Contract hub
- [Limits](/en/platform/limits) — `capabilities.limits` from the running binary
- [Compatibility](/en/platform/compatibility) — Contract, status semantics, member inventory
- [Deviations](/en/platform/deviations) — Registered single-machine differences (`OC-*`)

## Operate

Install `ocd`, write config, run it as a service, and the incident handbook: [ocd](/en/ocd/).

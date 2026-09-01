# Platform

open-compute runs the declared Cloudflare Workers programming model on a single node. The Worker API matches Cloudflare's documentation; storage, scheduling, and deploy live on this node.

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. When omitted, `limits` come from the embedded default config. Top-level JSON: `schema_version`, `release`, `runtime`, `products`, `limits`.

## Compatibility

| Area | Detail |
| --- | --- |
| Worker API | Worker-side symbols for Workers, KV, D1, R2, Durable Objects, Queues, Workflows, Cache, and Images match [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Signature index: [API reference](/en/platform/reference/api/). |
| Topology | Single node: one `ocd`, one pinned `workerd`, local authority storage. No global edge, dashboard, billing, or Cloudflare REST v4 / `client.v4`. |
| Project config | `open-compute.json`, not `wrangler.jsonc`. Unknown fields are rejected. |
| Limits | From `ocd capabilities --json` on the running binary. |
| Behavior differences | [Behavior differences](/en/platform/deviations). |
| Products not provided | [Unsupported](/en/platform/unsupported). |

## In this section

- [Compatibility](/en/platform/compatibility) — products, Worker APIs, single-node topology
- [Behavior differences](/en/platform/deviations) — TCP, limits, storage topology, and other behavior
- [Limits](/en/platform/limits) — live `capabilities.limits`
- [Unsupported](/en/platform/unsupported) — Cloudflare products that are not provided
- [API reference](/en/platform/reference/api/) — generated member-index entry

# Platform

open-compute runs the declared Cloudflare Workers programming model on a single node. The Worker API matches Cloudflare's documentation; storage, scheduling, and deploy live on this node.

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. When omitted, `limits` come from the embedded default config. Top-level JSON: `schema_version`, `release`, `runtime`, `products`, `limits`.

## Compatibility

| Area | Detail |
| --- | --- |
| Worker API | Worker-side symbols for Workers, KV, D1, R2, Durable Objects, Queues, Workflows, Cache, and Images match [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Signature index: [API reference](/platform/reference/api/). |
| Topology | Single node: one `ocd`, one pinned `workerd`, and local authority storage. Management uses the compatible `/client/v4` API and SDK-backed dashboard; global edge and billing are not provided. |
| Project config | Standard `wrangler.jsonc`, parsed by pinned `wrangler@4.127.1`; unsupported server capabilities fail closed. |
| Limits | From `ocd capabilities --json` on the running binary. |
| Behavior differences | [Behavior differences](/platform/deviations). |
| Products not provided | [Unsupported](/platform/unsupported). |

## In this section

- [Compatibility](/platform/compatibility) — products, Worker APIs, single-node topology
- [Behavior differences](/platform/deviations) — TCP, limits, storage topology, and other behavior
- [Limits](/platform/limits) — live `capabilities.limits`
- [Unsupported](/platform/unsupported) — Cloudflare products that are not provided
- [API reference](/platform/reference/api/) — generated member-index entry

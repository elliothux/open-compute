# Platform

The platform pages describe **the contract on this machine**: what is enabled, which differences are intentional, and which numeric limits apply. Do not infer full Cloudflare behavior from a product name. The authority is `ocd capabilities --json` on the running binary.

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. When omitted, `limits` come from the embedded default config. Top-level JSON: `schema_version`, `release`, `runtime`, `products`, `limits`.

## Same as Cloudflare

Worker-side symbols for Workers / KV / D1 / R2 / Durable Objects / Queues / Workflows / Cache / Images match the Cloudflare surface. Do not hand-copy member signatures here: use the [API reference](/en/platform/reference/api/) and the Cloudflare pages.

## Intentional differences

No global edge, no dashboard, no billing, no Cloudflare REST v4 / `client.v4`. The project file is `open-compute.json`, not `wrangler.jsonc`. Registered differences are `OC-*` IDs only: [Deviations](/en/platform/deviations). Non-target products: [Unsupported](/en/platform/unsupported).

## In this section

- [Compatibility](/en/platform/compatibility) — contract, status semantics, member inventory
- [Deviations](/en/platform/deviations) — the 15 registered `OC-*` IDs
- [Limits](/en/platform/limits) — live `capabilities.limits`
- [Unsupported](/en/platform/unsupported) — `non_target` products
- [API reference](/en/platform/reference/api/) — generated member-index entry

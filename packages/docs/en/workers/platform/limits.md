# Limits

Numeric ceilings come from `limits` in `ocd capabilities --json` on the node.

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

Without `--config`, `limits` come from the embedded default config. Cloudflare plan numbers: [Workers limits](https://developers.cloudflare.com/workers/platform/limits/). Do not infer that this process enforces those request-scoped quotas.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Programming model is isolates, not unbounded host processes | Yes | Yes |
| Product-specific durable limits (KV key size, D1 statements, R2 objects, cache object size, …) | Plan / product quota | Frozen in platform config and emitted in `limits` |
| Request-scoped CPU / subrequest / simultaneous-connection quotas | Yes | Stock OSS workerd `LimitEnforcer` does not enforce them; those hosted quotas are not provided. This does not relax the public-address security boundary, product-specific limits, handle cleanup, or process supervision |
| Cache object default | Cloudflare product quota | 16 MiB / 1 GiB logical body per Worker; see [Workers Cache](/en/workers/cache/) |


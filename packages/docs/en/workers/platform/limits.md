# Limits

Numeric ceilings come from `limits` in `ocd capabilities --json` on this machine. Do not copy this page or the default TOML as live quotas.

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

Without `--config`, `limits` come from the embedded default config. Cloudflare plan numbers: [Workers limits](https://developers.cloudflare.com/workers/platform/limits/). Do not infer that this process enforces those request-scoped quotas.

## Same as Cloudflare

Product-specific durable limits (KV key size, D1 statements, R2 objects, cache object size, …) are frozen in platform config and appear in `limits`. The programming model is still isolates, not unbounded host processes.

## Intentional delta: OC-WKR-LIMIT-001

The pinned stock OSS workerd standalone `LimitEnforcer` does not enforce subrequest or CPU limits. open-compute does not claim Cloudflare's request-scoped CPU, subrequest, or simultaneous-connection quotas. This does not relax the public-address security boundary, product-specific limits, handle cleanup, or process supervision.

Do not infer those hosted quotas from any other `limits` field. Cache object default 16 MiB / 1 GiB logical body per Worker: `OC-CACHE-002`.

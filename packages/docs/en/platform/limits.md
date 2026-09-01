# Limits

Limits come from the **running** binary: `limits` on `ocd capabilities --json`. Those are frozen product-specific numeric ceilings from config. **No secrets.** Do not copy this page, the default TOML, or Cloudflare's hosted docs as live quotas on this machine.

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

Without `--config`, `limits` come from the embedded default config.

## Hosted quotas you must not infer from `limits`

[`OC-WKR-LIMIT-001`](/en/platform/deviations#oc-wkr-limit-001): the pinned stock OSS workerd standalone `LimitEnforcer` does not enforce Cloudflare's hosted request-scoped CPU, subrequest, or simultaneous-connection quotas. Do not infer those quotas from any other `limits` field.

The hosted numbers (which we **do not claim**) are on [Workers platform limits](https://developers.cloudflare.com/workers/platform/limits/).

## Cache capacity

[`OC-CACHE-002`](/en/platform/deviations#oc-cache-002): the operator-configured default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. Exact live values still come from current `capabilities.limits`.

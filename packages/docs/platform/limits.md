# Limits

Limits come from the **running** binary: `limits` on `ocd capabilities --json`. Those are frozen product-specific numeric ceilings from config. **No secrets.**

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

Without `--config`, `limits` come from the embedded default config.

## Hosted quotas the runtime does not enforce

The pinned open-source `workerd` standalone process does not enforce Cloudflare's hosted request-scoped CPU, subrequest, or simultaneous-connection quotas. `LimitEnforcer` subrequest accounting is a no-op and `getLimitsExceeded()` always reports none. Do not infer those quotas from any other `limits` field.

Hosted numbers are on [Workers platform limits](https://developers.cloudflare.com/workers/platform/limits/). Behavior notes: [Behavior differences](/platform/deviations).

## Cache capacity

The default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. Exact live values still come from current `capabilities.limits`.

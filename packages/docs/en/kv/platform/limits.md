# Limits

Worker API ceilings match Cloudflare KV (512-byte keys, 25 MiB values, 1024-byte metadata, bulk get 100, list 1000, `expirationTtl` ≥ 60s). That is API shape, not a Cloudflare plan.

Local quotas come from the `ocd` `[kv]` config. **Live numbers** are `limits` from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `namespace_quota_bytes` | 1 GiB |
| `max_connections` | 64 |
| `max_readers_per_namespace` | 2 |
| `max_active_streams` | 16 |
| `operation_timeout_ms` | 30000 |

No Cloudflare billing, and no plan-scaled global KV quota. Do not copy plan limits from developers.cloudflare.com onto this machine.

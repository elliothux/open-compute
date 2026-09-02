# Limits

Local quotas come from `ocd` `[workflows]`. Live numbers come from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `max_steps` | 1024 |
| `max_state_bytes` | 32 MiB |
| `max_instances_per_account` | 10000 |
| `max_active_per_account` | 1000 |
| `max_parallel_steps` | 4 |
| `dispatch_timeout_ms` | 300000 |

A Cloudflare dashboard quota page is not provided.

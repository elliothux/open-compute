# Limits

Worker API shape matches the [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/). Local quotas come from `ocd` `[d1]`. Live numbers are `limits` from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `database_quota_bytes` | 1 GiB |
| `max_open_databases` | 32 |
| `max_queued_operations_per_database` | 64 |
| `max_result_rows` | 10000 |
| `max_result_bytes` | 8 MiB |
| `query_timeout_ms` | 30000 |
| `batch_timeout_ms` | 30000 |

Cloudflare plans, replica counts, and per-region capacity are not provided.

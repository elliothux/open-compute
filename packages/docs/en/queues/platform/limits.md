# Limits

Producer ceilings match the usual Cloudflare shape: 128 KiB per message, 100 / 256 KiB per batch, 86400s delay. Local quotas come from `ocd` `[queues]` and `[scheduler]`. **Live numbers** come from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `default_max_backlog_bytes` | 1 GiB |
| `max_in_flight_requests` | 64 |
| `max_in_flight_requests_per_binding` | 8 |
| `max_consumer_concurrency` | 32 |

No Cloudflare 250-consumer-concurrency plan promise.

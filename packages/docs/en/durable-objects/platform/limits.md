# Limits

Worker API shape matches the [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/). Local quotas come from `ocd` `[durable_objects]`. **Live numbers** come from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `max_namespace_name_bytes` | 128 |
| `max_object_name_bytes` | 1024 |
| `max_fetch_body_bytes` | 32 MiB |
| `dispatch_timeout_ms` | 30000 |
| `max_in_flight_dispatches` | 256 |

No Cloudflare global concurrency plan. Alarms and hibernation share this process's ceilings.

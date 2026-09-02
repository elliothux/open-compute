# Limits

Local quotas come from `ocd` `[images]`. Live numbers come from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `max_input_bytes` | 20 MiB |
| `max_output_bytes` | 20 MiB |
| `max_pixels` | 40_000_000 |
| `max_dimension` | 12000 |
| `max_operations` | 16 |
| `max_overlays` | 8 |
| `max_frames` | 1 |
| `max_sessions` | 64 |
| `max_concurrency` | 4 |
| `request_timeout_ms` | 10000 |

These are not Cloudflare Images plan quotas.

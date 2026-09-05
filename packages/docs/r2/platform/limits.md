# Limits

Worker API shape matches the [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/). Local quotas come from `ocd` `[r2]`. Live numbers come from `ocd capabilities --json`. Embedded defaults include:

| Item | Default |
| --- | --- |
| `max_object_bytes` | 512 MiB |
| `max_concurrent_uploads` | 4 |
| `max_concurrent_downloads` | 16 |
| `max_staging_bytes` | 2 GiB |
| `operation_timeout_ms` | 30000 |

This is not Cloudflare's 5 TiB object size or unlimited storage plan. Local free-space reserves or the selected S3 provider's limits also apply.

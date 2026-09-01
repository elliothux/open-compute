# 限额

Worker API 形状与 [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) 对齐。本机配额来自 `ocd` 的 `[r2]`。运行中精确值以 `ocd capabilities --json` 为准。嵌入默认包括：

| 项 | 默认 |
| --- | --- |
| `max_object_bytes` | 512 MiB |
| `max_concurrent_uploads` | 4 |
| `max_concurrent_downloads` | 16 |
| `max_staging_bytes` | 2 GiB |
| `operation_timeout_ms` | 30000 |

不是 Cloudflare 的 5 TiB 对象或无限存储套餐。S3 provider 自己的限额另外适用。

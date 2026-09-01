# 限额

Worker API 形状与 [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) 对齐。本机配额来自 `ocd` 的 `[d1]`，**运行中精确值**以 `ocd capabilities --json` 的 `limits` 为准。嵌入默认包括：

| 项 | 默认 |
| --- | --- |
| `database_quota_bytes` | 1 GiB |
| `max_open_databases` | 32 |
| `max_queued_operations_per_database` | 64 |
| `max_result_rows` | 10000 |
| `max_result_bytes` | 8 MiB |
| `query_timeout_ms` | 30000 |
| `batch_timeout_ms` | 30000 |

没有 Cloudflare 套餐、没有 replica 数量、没有按 region 的容量承诺。

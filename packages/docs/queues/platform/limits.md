# 限额

Producer 上限与 Cloudflare 常用形状对齐：单条 128 KiB、batch 100 / 256 KiB、delay 86400s。本机配额来自 `ocd` 的 `[queues]` 与 `[scheduler]`。运行中精确值以 `ocd capabilities --json` 为准。嵌入默认包括：

| 项 | 默认 |
| --- | --- |
| `default_max_backlog_bytes` | 1 GiB |
| `max_in_flight_requests` | 64 |
| `max_in_flight_requests_per_binding` | 8 |
| `max_consumer_concurrency` | 32 |

不提供 Cloudflare 的 250 consumer concurrency 套餐承诺。

# 限额

Worker API 形状与 [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) 对齐。本机配额来自 `ocd` 的 `[durable_objects]`。运行中精确值以 `ocd capabilities --json` 为准。嵌入默认包括：

| 项 | 默认 |
| --- | --- |
| `max_namespace_name_bytes` | 128 |
| `max_object_name_bytes` | 1024 |
| `max_fetch_body_bytes` | 32 MiB |
| `dispatch_timeout_ms` | 30000 |
| `max_in_flight_dispatches` | 256 |

不提供 Cloudflare 的全球并发套餐。Alarms 与 hibernation 走同一进程限额。

# 限额

本机配额来自 `ocd` 的 `[workflows]`。运行中精确值以 `ocd capabilities --json` 为准。嵌入默认包括：

| 项 | 默认 |
| --- | --- |
| `max_steps` | 1024 |
| `max_state_bytes` | 32 MiB |
| `max_instances_per_account` | 10000 |
| `max_active_per_account` | 1000 |
| `max_parallel_steps` | 4 |
| `dispatch_timeout_ms` | 300000 |

不提供 Cloudflare dashboard 配额页。

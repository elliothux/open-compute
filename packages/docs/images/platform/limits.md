# 限额

本机配额来自 `ocd` 的 `[images]`，**运行中精确值**以 `ocd capabilities --json` 为准。嵌入默认包括：

| 项 | 默认 |
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

这些不是 Cloudflare Images 套餐配额。不要引用 Cloudflare Images limits 页当本机合同。

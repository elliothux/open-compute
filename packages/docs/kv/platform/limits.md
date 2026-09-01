# 限额

Worker API 上限与 Cloudflare KV 对齐（键 512 字节、值 25 MiB、metadata 1024 字节、bulk get 100、list 1000、`expirationTtl` ≥ 60s）。这是 API 形状，不是 Cloudflare 套餐。

本机配额来自 `ocd` 配置的 `[kv]`，**运行中的精确值**以 `ocd capabilities --json` 的 `limits` 为准。嵌入默认包括：

| 项 | 默认 |
| --- | --- |
| `namespace_quota_bytes` | 1 GiB |
| `max_connections` | 64 |
| `max_readers_per_namespace` | 2 |
| `max_active_streams` | 16 |
| `operation_timeout_ms` | 30000 |

没有 Cloudflare 计费、没有按套餐放大的全球 KV 配额。不要把 developers.cloudflare.com 上的 plan limits 抄到这台机器。

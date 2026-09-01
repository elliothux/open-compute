# 偏差

登记 ID：**`OC-DO-001`**。

Durable Objects 落在本地这一个 workerd 进程上。location hint、jurisdiction 和全球迁移没有地理调度效果。

115 个目标成员因此是 `supported_with_deviation`（其中 112 个用 `OC-DO-001`；3 个 connect 成员额外带 `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001`）。Alarms（7）和 WebSocket hibernation（19）为 `supported`，没有单独偏差 ID。

见 [Compatibility](/platform/compatibility) 与 `docs/references/p1-deviations.md`。

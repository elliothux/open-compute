# 偏差

登记 ID：**`OC-D1-001`**。

D1 是单个本地主 SQLite authority，不声称 read replica、region routing、hosted `served_by` 身份、region/colo metadata 或 Cloudflare 计费计数。opaque bookmark 保证同一数据库的本地顺序可见性；`rows_read` / `rows_written` 是稳定的本地 SQLite 执行计数。

36 个目标成员因此是 `supported_with_deviation`。`dump()` 拒绝是实现托管非 alpha 行为，不是另设偏差 ID。没有 replicas。

见 [Compatibility](/platform/compatibility) 与仓库 `docs/references/p1-deviations.md`。

# 概念

每个 D1 database 是一份本地主 SQLite。所有 query 打到这一份权威。没有 replica 集合，所以也没有「读从库 / 写主库」分流。

session 与 opaque bookmark 仍然存在：bookmark 保证**同一数据库**上的本地顺序可见性，不是跨区域因果。`rows_read` / `rows_written` 是这次 SQLite 执行的稳定计数，不是 Cloudflare 计费。

`dump()` 对当前 hosted 非 alpha 模型拒绝（`D1_DUMP_ERROR`），与托管行为一致。

## 与 Cloudflare 相同

`prepare` → `bind` → `run` / `all` / `first` / `raw`；`batch` 顺序、原子；`exec` 跑无参数 SQL；`withSession` 接受 `"first-primary"` / `"first-unconstrained"` / bookmark 字符串。见 [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/)。

## 故意不同

**`OC-D1-001`**：没有 read replica、region routing、hosted `served_by` 身份、region/colo metadata 或 Cloudflare 计费计数。不要把 meta 里的 `served_by_*` 当成 colo 产品。

下一步：[指南](/d1/guides/)。

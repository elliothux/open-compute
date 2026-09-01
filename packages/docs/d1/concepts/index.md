# 概念

每个 D1 database 是一份本地主 SQLite。所有 query 打到这一份权威。不提供 replica 集合，因此也没有读从库 / 写主库分流。

session 与 opaque bookmark 仍然存在。bookmark 保证同一数据库上的本地顺序可见性，不是跨区域因果。`rows_read` / `rows_written` 是这次 SQLite 执行的稳定计数，不是 Cloudflare 计费。

`dump()` 对当前 hosted 非 alpha 模型拒绝（`D1_DUMP_ERROR`），与托管行为一致。

`prepare` → `bind` → `run` / `all` / `first` / `raw`。`batch` 顺序、原子。`exec` 跑无参数 SQL。`withSession` 接受 `"first-primary"` / `"first-unconstrained"` / bookmark 字符串。见 [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | 相同：`prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`、session、bookmark |
| Read replica / region routing | 提供 | 不提供 |
| Bookmark | 跨副本因果 | 同一数据库的本地顺序 |
| `rows_read` / `rows_written` | 计费计数 | 本地 SQLite 执行计数 |
| `dump()` | hosted 非 alpha 拒绝 | 同样拒绝 |

下一步：[指南](/d1/guides/)。

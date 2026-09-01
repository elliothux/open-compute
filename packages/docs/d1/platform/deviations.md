# 行为差异

D1 的 Worker API 与 Cloudflare 对齐；存储拓扑不同。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | 相同：`prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`、session、不透明 bookmark、prepared-statement / result / meta |
| 拓扑 | 托管 D1，含 read replica | 运行 ocd 的主机上的本地主 SQLite |
| Read replica | 提供 | 不提供 |
| Region routing | 提供 | 不提供 |
| `served_by` 地理 | region / colo metadata | 不提供；`served_by_*` 不是地理产品 |
| Bookmark | 跨副本因果 | 同一数据库的本地顺序 |
| `rows_read` / `rows_written` | 计费计数 | 本地 SQLite 执行计数 |
| `dump()` | hosted 非 alpha 拒绝 | 同样拒绝（`D1_DUMP_ERROR`） |
| REST / `client.v4` | 提供 | 不提供；使用 Worker binding |

见[兼容性](/platform/compatibility)。

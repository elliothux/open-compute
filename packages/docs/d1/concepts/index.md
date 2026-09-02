# Concepts

Each D1 database is a local-primary SQLite file. Every query hits that one authority. Read replicas and a read-replica / write-primary split are not provided.

Sessions and opaque bookmarks still exist. A bookmark preserves same-database local sequential visibility, not cross-region causality. `rows_read` / `rows_written` are stable counts for that SQLite execution, not Cloudflare billing.

`dump()` is rejected for the current hosted non-alpha model (`D1_DUMP_ERROR`), matching hosted behavior.

`prepare` → `bind` → `run` / `all` / `first` / `raw`. `batch` is sequential and atomic. `exec` runs parameter-free SQL. `withSession` accepts `"first-primary"` / `"first-unconstrained"` / a bookmark string. See the [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | Same: `prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`, sessions, bookmarks |
| Read replicas / region routing | Available | Not provided |
| Bookmarks | Cross-replica causality | Local ordering on the same database |
| `rows_read` / `rows_written` | Billing counters | Local SQLite execution counts |
| `dump()` | Rejected on hosted non-alpha | Rejected |

Next: [Guides](/d1/guides/).

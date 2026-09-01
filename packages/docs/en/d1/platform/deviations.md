# Behavior differences

The D1 Worker API matches Cloudflare. The storage topology does not.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | Same: `prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`, sessions, opaque bookmarks, prepared-statement / result / meta |
| Topology | Hosted D1 with read replicas | Local primary SQLite on the node running ocd |
| Read replicas | Available | Not provided |
| Region routing | Available | Not provided |
| `served_by` geography | Region / colo metadata | Not provided; `served_by_*` is not a geography product |
| Bookmarks | Cross-replica causality | Local ordering on the same database |
| `rows_read` / `rows_written` | Billing counters | Local SQLite execution counts |
| `dump()` | Rejected on hosted non-alpha | Rejected (`D1_DUMP_ERROR`) |
| REST / `client.v4` | Available | Not provided; use the Worker binding |

See [Compatibility](/en/platform/compatibility).

# Concepts

Each D1 database is a local-primary SQLite file. Every query hits that one authority. There is no replica set, so there is no read-replica / write-primary split.

Sessions and opaque bookmarks still exist: a bookmark preserves **same-database** local sequential visibility, not cross-region causality. `rows_read` / `rows_written` are stable counts for that SQLite execution, not Cloudflare billing.

`dump()` is rejected for the current hosted non-alpha model (`D1_DUMP_ERROR`), matching hosted behavior.

## Same as Cloudflare

`prepare` → `bind` → `run` / `all` / `first` / `raw`; `batch` is sequential and atomic; `exec` runs parameter-free SQL; `withSession` accepts `"first-primary"` / `"first-unconstrained"` / a bookmark string. See the [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/).

## Intentional differences

**`OC-D1-001`**: no read replicas, region routing, hosted `served_by` identity, region/colo metadata, or Cloudflare billing counters. Do not treat `served_by_*` in meta as a colo product.

Next: [Guides](/en/d1/guides/).

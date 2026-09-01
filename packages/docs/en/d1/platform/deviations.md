# Deviations

Registered ID: **`OC-D1-001`**.

D1 is a single local-primary SQLite authority. The platform does not claim read-replica/region routing, hosted `served_by` identity, region/colo metadata, or Cloudflare billing counters. Opaque bookmarks preserve same-database local sequential visibility; `rows_read` and `rows_written` are stable local SQLite execution counters.

That is why 36 target members are `supported_with_deviation`. Rejecting `dump()` is hosted non-alpha behavior, not a second deviation ID. There are no replicas.

See [Compatibility](/en/platform/compatibility) and `docs/references/p1-deviations.md`.

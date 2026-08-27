# P1 compatibility deviations

This file owns the stable deviation identifiers emitted by `platformd capabilities --json`.

- `OC-KV-001`: KV is single-node SQLite authority; it does not claim Cloudflare global replication or propagation timing.
- `OC-R2-001`: R2 is backed by the configured S3 authority; a full platform snapshot records bucket identity but does not provide R2 point-in-time recovery.
- `OC-D1-001`: D1 session constraints and bookmark replication are not implemented; `withSession()` only exposes the explicitly documented local behavior.
- `OC-DO-001`: Durable Objects are placed on the single local workerd process; placement hints and global migration are unsupported.
- `OC-WS-001`: Basic Durable Object WebSocket is supported. Native hibernatable WebSocket remains disabled until the pinned stock-workerd hard Gate is a complete Go.
- `OC-QUEUE-001`: Queue producers and push consumers are backed by single-node `scheduler.sqlite` durability, not Cloudflare global replication. Delivery is at-least-once without strict FIFO; an unknown native dispatch retains its lease and does not consume the tenant retry budget, so a later delivery can repeat the same attempt number. JSON is the default for every supported compatibility date; `v8`, metadata, pull consumers, multiple consumers per Queue, resource-level PITR, and Cloudflare plan quotas are unsupported. Durable Object Queue writes remain fail closed with `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED` because the service-facade transport cannot inherit stock workerd's native Durable Object output gate.
- `OC-CRON-001`: Cron is UTC-only with five fields and the documented local Quartz-like extensions. Recovery projects at most the newest slot within the configured misfire grace rather than replaying complete downtime history; known failures use the configured bounded local retry policy unless `noRetry()` is called.

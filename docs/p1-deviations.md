# P1 compatibility deviations

This file owns the stable deviation identifiers emitted by `platformd capabilities --json`.

- `OC-KV-001`: KV is single-node SQLite authority; it does not claim Cloudflare global replication or propagation timing.
- `OC-R2-001`: R2 is backed by the configured S3 authority; a full platform snapshot records bucket identity but does not provide R2 point-in-time recovery.
- `OC-D1-001`: D1 session constraints and bookmark replication are not implemented; `withSession()` only exposes the explicitly documented local behavior.
- `OC-DO-001`: Durable Objects are placed on the single local workerd process; placement hints and global migration are unsupported.
- `OC-WS-001`: Basic Durable Object WebSocket is supported. Native hibernatable WebSocket remains disabled until the pinned stock-workerd hard Gate is a complete Go.
- `OC-QUEUE-001`: Queue producers are supported in ordinary Workers and named WorkerEntrypoints. The capability is single-node `scheduler.sqlite` durability, not Cloudflare global replication. JSON is the default for every supported compatibility date; `v8` and metadata are unsupported. P2.2 has no consumer, DLQ, Cron, exactly-once retry, resource-level PITR, or Cloudflare plan quota. A lost producer response may have committed and retry may duplicate. Queue data participates only in the complete offline platform snapshot. Durable Object Queue writes fail closed with `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED` because the service-facade transport cannot inherit stock workerd's native Durable Object output gate.

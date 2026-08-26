# P1 compatibility deviations

This file owns the stable deviation identifiers emitted by `platformd capabilities --json`.

- `OC-KV-001`: KV is single-node SQLite authority; it does not claim Cloudflare global replication or propagation timing.
- `OC-R2-001`: R2 is backed by the configured S3 authority; a full platform snapshot records bucket identity but does not provide R2 point-in-time recovery.
- `OC-D1-001`: D1 session constraints and bookmark replication are not implemented; `withSession()` only exposes the explicitly documented local behavior.
- `OC-DO-001`: Durable Objects are placed on the single local workerd process; placement hints and global migration are unsupported.
- `OC-WS-001`: Basic Durable Object WebSocket is supported. Native hibernatable WebSocket remains disabled until the pinned stock-workerd hard Gate is a complete Go.

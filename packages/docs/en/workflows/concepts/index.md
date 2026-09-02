# Concepts

A Workflow definition is a catalog resource. Instance and step state live in local SQLite. When `run` is replayed, `step.do` callbacks that already committed are skipped; uncommitted callbacks may run again (at-least-once until commit).

Side effects on KV / R2 / external HTTP do **not** roll back with a Workflow snapshot. Make external writes idempotent.

Cross-region placement and Workflow observability in a Cloudflare dashboard are not provided.

[Workflows](https://developers.cloudflare.com/workflows/) class / `step.do` / instance handle match Cloudflare. Bounded parallel `step.do` (default at most 4) is implemented behavior.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| API | [Workflows](https://developers.cloudflare.com/workflows/) | Same: class / `step.do` / instance handle |
| Execution | Cross-region | Local SQLite on the node running ocd |
| Callbacks | — | At-least-once until commit; completed callbacks skip on replay |
| External side effects | — | Do not roll back with the snapshot |

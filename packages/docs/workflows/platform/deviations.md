# Behavior differences

The Workflows binding / instance API matches Cloudflare. Execution authority is local SQLite on this node.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Binding / instance API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | Same: `create` / `get` / `createBatch` / `deleteBatch`, `step.do` / sleep / event, status / pause / resume / terminate / restart |
| Execution | Cross-region | Local SQLite on the node running ocd |
| Callbacks | — | At-least-once until result commit; replay skips durable-complete callbacks |
| External side effects | — | Do not roll back with Workflow snapshots |
| Dashboard | Available | Lifecycle controls use the official SDK-backed dashboard |
| Binding | Wrangler | Standard `binding`, `name`, `class_name`, and optional `schedules` |

Batch / rollback / structured-clone / parallel are implemented behavior.

See [Compatibility](/platform/compatibility).

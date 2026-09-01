# Concepts

A Workflow definition is a catalog resource. Instance and step state live in local SQLite. When `run` is replayed, `step.do` callbacks that already committed are skipped; uncommitted callbacks may run again (at-least-once until commit).

Side effects on KV / R2 / external HTTP do **not** roll back with a Workflow snapshot. Make external writes idempotent.

There is no cross-region placement and no Workflow observability in a Cloudflare dashboard (`OC-WORKFLOW-001`).

## Same as Cloudflare

[Workflows](https://developers.cloudflare.com/workflows/) class / `step.do` / instance handle. Bounded parallel `step.do` (default at most 4) is implemented behavior, not a deviation.

## Intentional differences

**`OC-WORKFLOW-001`**: local SQLite; callbacks at-least-once until commit; external side effects do not roll back; no cross-region, no CF dashboard/observability.

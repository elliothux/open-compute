# Deviations

Registered ID: **`OC-WORKFLOW-001`**.

Workflow execution uses local SQLite authority. Callbacks are at-least-once until their result commits; replay skips durably completed callbacks; external product effects do not roll back with Workflow snapshots. The platform does not claim cross-region execution, global placement, or Cloudflare dashboard/observability.

That is why 72 target members are `supported_with_deviation`. Batch / rollback / structured-clone / parallel are implemented behavior, not deviations.

See [Compatibility](/en/platform/compatibility) and `docs/references/p1-deviations.md`.

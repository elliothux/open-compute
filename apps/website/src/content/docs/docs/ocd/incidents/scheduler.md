---
title: "Scheduler recovery"
---

Trigger: due lag, expired lease, repair backlog, or scheduler DB not inspectable. Blast radius includes Alarm, Queue, Cron, Workflow; ordinary Worker/DO fetch may be degraded.

Read-only diagnosis:

```sh
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml doctor --json
```

Prefer waiting for token/expiry recovery and bounded repair. Inspect scheduler state through `/client/v4/open-compute/scheduler` and Workflow instances through the official account Workflows API. An Unknown Workflow dispatch keeps its lease; do not treat it as a business failure you can retry immediately.

A Cron claim consumes its one-based delivery attempt before dispatch. Transport-unknown outcomes retain the claim until lease expiry, then retry only while both the configured retry budget and the fixed 15-minute handler deadline remain. Draining an activation terminalizes ready runs immediately and never requeues an expired claim. Cron inspection reports the last error, last unknown classification, and oldest live deadline, so one unknown result cannot block later deployments indefinitely.

Queue consumer and Cron activation dispatch epochs are frozen in the scheduler projection. Adding or editing an HTTP route does not replace them. Retrying promotion or starting reconcile reuses that epoch and still strictly validates target, descriptor, and product generation. Do not write the current Worker route revision back into an already created projection or claim.

If scheduler history is corrupt, stop the service and restore a full-platform snapshot with [fresh-host restore](/docs/ocd/incidents/fresh-host/). Restore does not undo external side effects. Do not delete referrer, operation, receipt, or step rows to bypass checks.

Workflow waiting/paused does not consume execution concurrency. The official Workflow instance response exposes lifecycle state and steps. Retention history metrics such as `workflow_*_results` and `workflow_consumed_events` are gauges; restart/purge can lower them. `workflow_event_intake_total` and `workflow_lifecycle_total` are this process's observed call results. The fixed metrics budget now needs at least 567 series; the default remains 1024.

If the runtime cannot confirm callback drain, the current Workflow execution path isolates the current workerd generation. Operator resume cannot lift that isolation; only a supervisor-started new generation can admit Workflow again. Existing leases stay for Unknown recovery and do not increment attempt as a business retry.

Scheduler tables share `control.sqlite` with the control plane. If the database is corrupt, stop the service and restore a full-platform snapshot. `scheduler recover-corrupt` refuses file replacement because it would also replace control state.

At upgrade, startup imports an existing `scheduler.sqlite` in one transaction. The transaction also records completion in `platform_meta`. Startup retains the old file as `scheduler.sqlite.imported`, with any remaining WAL and SHM sidecars. A restart completes an interrupted import or rename without duplicating rows. If control already contains scheduler rows without the marker, startup stops instead of merging them.

New snapshots store these tables in `control.sqlite` only. Restore an older snapshot with its source release, then upgrade; startup imports the restored scheduler file. Keep the imported copy until the upgraded instance and its snapshot have been verified.

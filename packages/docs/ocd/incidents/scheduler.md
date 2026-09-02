# Scheduler recovery

Trigger: due lag, expired lease, repair backlog, or scheduler DB not inspectable. Blast radius includes Alarm, Queue, Cron, Workflow; ordinary Worker/DO fetch may be degraded.

Read-only diagnosis:

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
```

Prefer waiting for token/expiry recovery and bounded repair. Inspect pools via `/v1/scheduler` and `/v1/operator/workflows` first. An Unknown Workflow dispatch keeps its lease; do not treat it as a business failure you can retry immediately.

Queue consumer and Cron activation dispatch epochs are frozen in the scheduler projection. Adding or editing an HTTP route does not replace them. Retrying promotion or starting reconcile reuses that epoch and still strictly validates target, descriptor, and product generation. Do not write the current Worker route revision back into an already created projection or claim.

If control still has a Queue, Cron activation, Workflow instance (including released/terminal/retained), Workflow operation, or Workflow version, you cannot rebuild scheduling history from an empty database. After Workflow purge releases control references, scheduler may still hold GC receipts; a corrupt file cannot prove those records are absent. Stop the service and restore a full-platform snapshot with [fresh-host restore](/ocd/incidents/fresh-host). That does not undo external side effects that already happened. Do not delete referrer, operation, receipt, or step rows to bypass the check.

Workflow waiting/paused does not consume execution concurrency. `/v1/operator/workflows` includes waiting, inbox, retention, and operation counts. Retention history metrics such as `workflow_*_results` and `workflow_consumed_events` are gauges; restart/purge can lower them. `workflow_event_intake_total` and `workflow_lifecycle_total` are this process's observed call results. The fixed metrics budget now needs at least 567 series; the default remains 1024.

If the runtime cannot confirm callback drain, the current Workflow execution path isolates the current workerd generation. Operator resume cannot lift that isolation; only a supervisor-started new generation can admit Workflow again. Existing leases stay for Unknown recovery and do not increment attempt as a business retry.

Allowed mutation: the following command is only for an **alarm-only data directory whose control plane is verifiable and that has none of the product authority above**, and only after the scheduler DB is confirmed corrupt and the service is stopped. The command refuses directories that still hold product authority before it moves files:

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml scheduler recover-corrupt --backup-name scheduler-corrupt-20260826
```

`--backup-name` is a unique directory created under `data/diagnostics/scheduler-recovery/`. Expect the old DB to be isolated exactly, an empty projection repaired from DO alarm authority, and no fabricated deliveries. Stop conditions: control/DO authority also corrupt, unknown tokens double-committed, or backlog not converging — then full-platform restore. Rollback is keeping the isolated copy and restoring a full snapshot. Verification: repair dry-run, alarm sentinel, lease expiry, restart, and lag back within bounds.

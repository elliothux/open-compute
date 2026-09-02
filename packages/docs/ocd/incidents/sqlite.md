# SQLite corruption

Trigger: doctor reports quick_check, migration checksum, foreign-key, or schema-tuple failure. Blast radius depends on whether the file is control, scheduler, KV, or D1; control corruption is a whole-platform incident.

Read-only diagnosis: stop the service, then:

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup list --json
```

Do not edit SQLite, WAL, SHM, or migration tables directly. The allowed mutation is a [fresh-host restore](/ocd/incidents/fresh-host) from the newest verified full-platform snapshot. Only an alarm-only directory whose control plane is verifiable and that has no Queue, Cron activation, Workflow history, operation, or version may use `scheduler recover-corrupt --backup-name scheduler-corrupt-20260826` to rebuild the projection. A Workflow catalog also blocks rebuild because the corrupt file may still hold purge receipts; see [Scheduler recovery](/ocd/incidents/scheduler).

Expect the corrupt file to stay isolated and authority not to be rewritten by self-healing. Stop conditions: no committed snapshot, no master key, or not the same S3 authority — preserve the files immediately. Rollback is returning to the pre-restore read-only evidence. Verification: doctor full, P0 smoke, schema checksum, and one restart.

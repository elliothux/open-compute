---
title: "SQLite corruption"
---

Trigger: doctor reports quick_check, Refinery-history, foreign-key, or owned-schema failure. Blast radius depends on whether the file is control, scheduler, KV, or D1; control corruption is a whole-platform incident.

Read-only diagnosis: stop the service, then:

```sh
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml doctor --json
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml backup list --json
```

Do not edit SQLite, WAL, SHM, or migration tables directly. The allowed mutation is a [fresh-host restore](/docs/ocd/incidents/fresh-host/) from the newest verified full-platform snapshot. Scheduler tables share `control.sqlite`; `scheduler recover-corrupt` refuses file replacement. See [Scheduler recovery](/docs/ocd/incidents/scheduler/).

Expect the corrupt file to stay isolated and authority not to be rewritten by self-healing. Stop conditions: no committed snapshot, no master key, or not the same object authority — preserve the files immediately. Rollback is returning to the pre-restore read-only evidence. Verification: doctor full, P0 smoke, every authoritative file's Refinery history and owned schema, and one restart.

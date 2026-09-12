---
title: "Operate open-compute"
description: "Configure, monitor, back up, upgrade, and recover one open-compute host."
---

One `ocd` process owns one platform configuration, one data directory, one SQLite authority, one Local or S3 object authority, and one supervised pinned workerd child. Never run two instances against the same data directory.

## Daily operation

```sh
ocd instances
ocd status
ocd logs --follow
ocd dashboard
ocd restart
```

Use `--instance <id>` when several registered instances exist, or `--config <absolute-path>` to select an exact configuration. The two selectors are mutually exclusive.

`/health/live` reports process liveness. `/health/ready` reports admission readiness. Diagnose a failed ready check before restarting:

```sh
ocd doctor
ocd doctor --full
ocd capabilities --json
```

`doctor --full` is an explicit mutation: it performs an object-storage canary and a temporary runtime compile/start/stop check.

## Configuration and data

The recommended system setup uses `/etc/open-compute/config.toml` and `/var/lib/open-compute`. Secrets are references to environment variables or owner-only files, never inline values. Local object storage is the default; S3 is an explicit alternative authority and is not a runtime fallback.

Read [platform configuration](/docs/ocd/configuration/) before exposing listeners or selecting S3.

## Backup, upgrade, and recovery

Backups are offline, authenticated full-platform snapshots. Stop the instance, create and verify a snapshot, and rehearse restore into a fresh data directory. See [Backup and retention](/docs/ocd/backup/) and the [incident handbook](/docs/ocd/incidents/).

```sh
ocd upgrade --dry-run
ocd upgrade
```

Upgrade validates registered configurations and restarts only instances that were active. `ocd uninstall` removes receipt-owned installation files but never deletes platform configuration, secrets, or data.

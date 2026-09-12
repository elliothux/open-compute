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

`ocd stop` is synchronous at the operator boundary: it returns `INSTANCE_STOPPED` only after the service manager reports inactive, the control socket is gone, and the data-directory lock can be acquired. Failure to quiesce within the bounded timeout is an error, so a successful immediate `doctor` or `start` cannot race the prior process.

Use `--instance <id>` when several registered instances exist, or `--config <absolute-path>` to select an exact configuration. The two selectors are mutually exclusive.

`/health/live` reports process liveness. `/health/ready` reports admission readiness. Diagnose a failed ready check before restarting:

```sh
ocd doctor
ocd doctor --full
ocd capabilities --json
```

`doctor --full` is an explicit mutation: it performs an object-storage canary and a temporary runtime compile/start/stop check.

## Configuration and data

The default setup is user-owned. Linux uses `$XDG_CONFIG_HOME/open-compute/config.toml` and `$XDG_DATA_HOME/open-compute`, with `~/.config` and `~/.local/share` fallbacks. macOS uses `~/Library/Application Support/open-compute/config.toml` and its `data` child. `ocd setup --system --yes` is the explicit system alternative using `/etc/open-compute/config.toml` and `/var/lib/open-compute`. Root execution without `--system` is rejected.

Secrets are references to environment variables or owner-only files, never inline values. Local object storage is the default; S3 is an explicit alternative authority and is not a runtime fallback.

Read [platform configuration](/docs/ocd/configuration/) before exposing listeners or selecting S3.

## Backup, upgrade, and recovery

Backups are offline, authenticated full-platform snapshots. Stop the instance, create and verify a snapshot, and rehearse restore into a fresh data directory. See [Backup and retention](/docs/ocd/backup/) and the [incident handbook](/docs/ocd/incidents/).

```sh
ocd upgrade --dry-run
ocd upgrade
```

Upgrade validates registrations owned by the current executable and restarts only instances that were active. A stopped registration with a missing, changed, or invalid config is reported with its ID, path, error code, and exact `instance unregister` command but does not block binary replacement. An invalid active registration fails before replacement and reports the same actionable identity. `ocd uninstall` stops and unregisters only owned instances, removes the receipt-owned program, and prints every retained config, data, and object path. Registrations belonging to another executable are untouched.

Data removal is always explicit and irreversible:

```sh
ocd purge --instance <id> --dry-run
ocd purge --instance <id> --yes
ocd --config /exact/path/config.toml purge --yes
ocd uninstall --purge --yes
```

Purge prints and validates the complete plan before stopping the service. It rejects changed configuration, symlinks, root/home targets, hard-linked or special entries, live control sockets, and roots shared or overlapping with another registered instance. Local object roots are deleted only when uniquely owned; an S3 authority is always retained and reported for manual handling.

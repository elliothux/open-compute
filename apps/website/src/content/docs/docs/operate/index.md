---
title: "Operate open-compute"
description: "Configure, monitor, back up, upgrade, and recover one open-compute host."
---

One scoped `ocd run` process can own multiple explicitly registered instances. Each instance keeps its own configured data directory, SQLite authority, object authority, and supervised pinned workerd child; data directories cannot overlap.

## Daily operation

```sh
ocd instances
ocd status
ocd logs --follow
ocd restart
```

`ocd status` reports the selected user or system daemon's liveness. `ocd instances` reads live instance states from its control socket; when the daemon is offline, it lists explicit manifest entries as stopped. A held scope lock with an unavailable control socket is an error, not a stopped result.

Use `ocd instance start|stop|restart <id-or-name>` to change one registered instance without changing its `autostart` intent. `status` and `instances` select only the current user or explicit `--system` scope; they do not accept `--instance` or `--config`.

Instance-scoped commands require an explicit selector when more than one instance is registered. For example, open the Dashboard with `ocd --instance <id-or-name> dashboard`. See [Instances](/docs/ocd/instances/) and [Dashboard](/docs/ocd/dashboard/).

`/health/live` reports process liveness. `/health/ready` reports admission readiness. Diagnose a failed ready check before restarting:

```sh
ocd doctor
ocd doctor --full
ocd capabilities --json
```

`doctor --full` is an explicit mutation: it performs an object-storage canary and a temporary runtime compile/start/stop check.

## Configuration and data

The default setup is user-owned at `~/.open-compute/instances/default/compute.toml`, with explicit `[data].path` pointing to its `data` sibling. `ocd setup --system --yes` is the explicit system alternative at `/var/lib/open-compute/instances/default/compute.toml`. Both register the exact config in their scope's `ocd.toml`; `instances/` is not a discovery mechanism. Root execution without `--system` is rejected.

Secrets are references to environment variables or owner-only files, never inline values. Local object storage is the default; S3 is an explicit alternative authority and is not a runtime fallback.

Read [platform configuration](/docs/ocd/configuration/) before exposing listeners or selecting S3. Public routing is owned by the shared [Gateway](/docs/gateway/). Local native extensions are registered per instance and documented under [Extensions](/docs/extension/).

## Backup, upgrade, and recovery

Backups are offline, authenticated full-platform snapshots. Stop the instance, create and verify a snapshot, and rehearse restore into a fresh data directory. See [Backup and retention](/docs/ocd/backup/) and the [incident handbook](/docs/ocd/incidents/).

```sh
ocd upgrade --dry-run
ocd upgrade
```

Upgrade downloads and verifies the target, then asks that staged target binary to validate every active registered config and migrate a read-only SQLite snapshot before replacing the installed binary. It retains digest-bound binary and receipt backups until restart/readiness succeeds; a normal restart failure restores the prior release. If interruption leaves a backup, later upgrade attempts fail closed until the operator runs `ocd upgrade --restore`.

Upgrade and uninstall operate on every explicit registration in the selected daemon scope. A missing or invalid registered config makes `ocd.toml` invalid and fails closed before binary mutation because the manifest intentionally contains no duplicate identity or data fallback. `ocd uninstall` removes the receipt-owned program after unregistering those instances and preserves config and data unless purge is explicit.

Data removal is always explicit and irreversible:

```sh
ocd purge --instance <id> --dry-run
ocd purge --instance <id> --yes
ocd --config /exact/path/config.toml purge --yes
ocd uninstall --purge --yes
```

Purge prints and validates the complete plan while the scoped daemon is offline. It rejects changed configuration, symlinks, root/home targets, hard-linked or special entries, active daemon ownership, and data roots shared or overlapping with another registered instance. Local objects live inside the instance data root and are removed with it; an S3 authority is always retained and reported for manual handling.

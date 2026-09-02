# Operator overview

`ocd` is the open-compute platform process: it supervises a pinned `workerd` on a single node and exposes the control plane and the data plane.

## Artifact

The issued artifact is one file: an `ocd` matching the OS/CPU. workerd, system Workers, the default config, and the operator runbooks are embedded. The running process will not download a runtime, and you must not place a separate workerd next to it.

One `ocd` owns one data-dir and one workerd child. Do not start a second `ocd` on the same data-dir.

## What you still provide

- An absolute-path configuration file
- A locally writable data-dir (SQLite, identity, master key, and runtime extraction live here)
- An S3-compatible store for R2, Worker bundles, Static Assets, and large objects

Secrets travel only as `env:` / `file:` references in config. Do not put them in units, images, or the repository.

## Do not assume

- A single-file artifact still extracts and verifies the embedded runtime inside the data-dir on first run.
- Backups cover local SQLite authority. R2 stays bound to the bucket you configured; it is not object-store point-in-time recovery.
- `/health/live` only means the process is alive. `/health/ready` is admission; do not restart from a readiness failure.
- Tenants can only touch bindings declared in the deployment. They do not get SQLite paths, S3 credentials, or anyone else's resources.

## In this section

- [Install and first start](/ocd/get-started)
- [Configuration](/ocd/configuration)
- [Deploy](/ocd/deploy)
- [Health checks](/ocd/health)
- [Backup and retention](/ocd/backup)
- [CLI reference](/ocd/cli)
- [Incident handbook](/ocd/incidents/)

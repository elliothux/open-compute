# Overview

open-compute gives you a Workers platform on one machine: a single `platformd` process, with a pinned `workerd`, exposing the control plane and the data plane.

It is compatible with the declared subset of the Workers programming model (Worker, KV, R2, D1, Durable Objects, Queues, Cron, Workflows, Static Assets, Service Binding, Cache, Images). Compatibility is not "same name, same behavior". Global edge, cross-region replication, multi-replica high availability, billing, and the full Cloudflare management plane are out of scope. What is actually enabled, and which differences are intentional, is defined by `platformd capabilities --json` on that machine.

## What you get

The issued artifact is one file: a `platformd` matching the OS/CPU. workerd, system Workers, the default config, and the operator runbooks are embedded. The running process will not download a runtime, and you must not place a separate workerd next to it.

The process boundary is fixed: one `platformd` owns one data-dir and one workerd child. Do not start a second `platformd` on the same data-dir.

## What you still provide

- An absolute-path configuration file
- A locally writable data-dir (SQLite, identity, master key, and runtime extraction live here)
- An S3-compatible store for R2, Worker bundles, Static Assets, and large objects

Secrets travel only as `env:` / `file:` references in config. Do not put them in units, images, or the repository.

## Do not assume

- A single file does not mean "the disk will ever contain only that file". The first run extracts and verifies the embedded runtime inside the data-dir.
- Backups cover local SQLite authority. R2 stays bound to the bucket you configured; it is not object-store PITR.
- `/health/live` only means the process is alive. `/health/ready` is admission; do not restart from a readiness failure.
- Tenants can only touch bindings declared in the deployment. They do not get SQLite paths, S3 credentials, or anyone else's resources.

Next: install this one file and complete the first start. When something breaks, do not start in the source tree; go to the [incident handbook](/en/incidents/).

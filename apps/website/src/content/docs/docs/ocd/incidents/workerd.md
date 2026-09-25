---
title: "workerd crash loop"
---

Trigger: restart counter keeps climbing, readiness runtime unavailable, mass activation or WebSocket failures. Blast radius is tenant execution; the ocd control plane should still be alive.

Read-only diagnosis:

```sh
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml doctor --json
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml support-bundle --output /tmp/open-compute-support.tar
```

Inspect `deployment-runtime.json` and `workerd-last-exit.json` in the bundle. The latter retains only the latest bounded redacted stdout/stderr tails, exit code/signal, exact exited startup generation, restart reason, digest, and deployment-attribution class. `deployment_quarantined` means one in-flight active deployment was identified and rolled back; `attribution_ambiguous` or `unattributed` requires correlating the affected requests without manually changing SQLite.

Allowed mutation: stop the service, restore verified workerd/runtime assets from the **same** release package, then start. Do not search `PATH`, auto-download, or widen an abort allowlist. Replace and verify the complete `ocd`; do not swap cached workerd or JS by themselves.

Expect bounded supervisor backoff, reaping the old process group, old-generation tokens becoming invalid, and an exactly attributed deployment becoming permanently quarantined. Stop conditions: digest/version mismatch, unknown orphan identity, or localDisk compatibility not passing. Rollback is the complete old package plus its snapshot, not a single binary swap. Verification: doctor full, current runtime Gate, DO/alarms/basic WebSocket, and no orphan/FD leaks.

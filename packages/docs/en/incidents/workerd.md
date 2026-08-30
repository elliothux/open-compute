# workerd crash loop

Trigger: restart counter keeps climbing, readiness runtime unavailable, mass activation or WebSocket failures. Blast radius is tenant execution; the platformd control plane should still be alive.

Read-only diagnosis:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml capabilities --json
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --json
```

Allowed mutation: stop the service, restore verified workerd/runtime assets from the **same** release package, then start. Do not search `PATH`, auto-download, or widen an abort allowlist. Replace and verify the complete `platformd`; do not swap cached workerd or JS by themselves.

Expect bounded supervisor backoff, reaping the old process group, and old-generation tokens becoming invalid. Stop conditions: digest/version mismatch, unknown orphan identity, or localDisk compatibility not passing. Rollback is the complete old package plus its snapshot, not a single binary swap. Verification: doctor full, G0/P0 runtime smoke, DO/alarms/basic WebSocket, and no orphan/FD leaks.

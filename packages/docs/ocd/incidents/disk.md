# Disk pressure

Trigger: readiness reason is `DISK_SOFT` or `DISK_HARD`, `platform_disk_emergency_headroom_bytes` is near zero, or a mutation returns `storage_pressure`/507. Blast radius is anything that can grow local state: Worker, KV, D1, DO, object staging, and snapshots. With the Local object backend, the object root may be on a different filesystem from the data directory and is measured separately.

Read-only diagnosis:

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
/usr/bin/df -k /var/lib/open-compute
# Also inspect the configured Local object root when [storage].backend = "local".
```

Allowed mutations are only delete/GC of known owners, exact snapshot delete, and growing the affected filesystem. Stop large uploads first. Expect new writes to fail closed under hard pressure while reads, doctor, and emergency cleanup stay bounded. Unidentifiable file owner, read-only filesystem, SQLite I/O error, or zero emergency headroom are stop conditions. Do not delete DB, WAL, DO, Local object envelopes, multipart journals, marker files, or lock files. Rollback is reversing a capacity-policy change, not restoring deleted tenant data. Verification: both applicable filesystems have recovered headroom, doctor passes, 507 is gone, and staging/reservation is back to steady state.

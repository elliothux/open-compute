# Disk pressure

Trigger: readiness reason is storage pressure, `platform_disk_emergency_headroom_bytes` is near zero, or a mutation returns `storage_pressure`/507. Blast radius is anything that can grow local state: Worker, KV, D1, DO, R2 staging, and snapshots.

Read-only diagnosis:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --json
/usr/bin/df -k /var/lib/open-compute
```

Allowed mutations are only delete/GC of known owners, exact snapshot delete, and growing the filesystem that holds `/var/lib/open-compute`. Stop large uploads first. Expect new writes to fail closed under hard pressure while reads, doctor, and emergency cleanup stay bounded. Unidentifiable file owner, read-only filesystem, SQLite I/O error, or zero emergency headroom are stop conditions. Do not delete DB, WAL, DO, or lock files. Rollback is reversing a capacity-policy change, not restoring deleted tenant data. Verification: headroom recovered, doctor passes, 507 gone, staging/reservation back to steady state.

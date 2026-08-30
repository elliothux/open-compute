# Fresh-host restore

Trigger: original data-dir is gone, or a full disaster-recovery drill. Blast radius is the entire platform; R2 sees the provider's current state.

Read-only diagnosis: install the snapshot's exact source release, provide the same master key at `/etc/open-compute/recovery-master.key`, and confirm `/var/lib/open-compute` is missing or empty:

```sh
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml capabilities --json
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

Allowed mutation:

```sh
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml backup restore --snapshot 0198f000-0000-7000-8000-000000000001 --json
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml doctor --full --json
```

A snapshot that includes Workflow must keep control/scheduler authority, restart/purge intent, operation progress, and GC receipts together. Original waiting/paused deadlines, inbox, and frozen retention are not recomputed. After restore, let the exact-release reconciler finish legal intermediate states, then verify original-version replay, paused state, events, and due work. Do not copy one database without the other, and do not delete operation rows to make diagnostics green.

Expect sibling staging, full verification, then one atomic install. Non-empty target, wrong key/release/S3, path, hash, schema, or marker errors are stop conditions. Do not force or overwrite an old directory. On failure the target stays empty; the parent keeps a bounded `restore-failure` receipt and object staging under the same UUIDv7. After the diagnostic bytes are no longer needed, clean only the ID reported by that receipt:

```sh
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml backup cleanup-restore --staging 0198f000-0000-7000-8000-000000000002 --json
```

Rollback is: keep the target empty and fix key/release/S3/config. Cleanup refuses symlinks, hardlinks, non-regular files, non-manifest restore paths, and trees over the hard cap.

Verification: start the exact release, read KV/D1/DO/alarm sentinels, check deployment pins, basic WebSocket reconnect, a new write, and a second restart. After every step passes, stop the service again and record operator attestation. That command re-verifies snapshot, release, master key, platform identity, and the original restore receipt; it does not replace the product smoke above:

```sh
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml backup attest-restore-smoke --snapshot 0198f000-0000-7000-8000-000000000001 --passed --json
```

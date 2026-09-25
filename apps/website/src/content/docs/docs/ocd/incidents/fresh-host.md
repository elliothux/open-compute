---
title: "Fresh-host restore"
---

Trigger: an instance data directory is gone, or a full disaster-recovery drill. R2 sees the selected object authority's current state. First restore the selected scope's `ocd.toml`, global keys, persistent Gateway state, and every referenced `compute.toml` at its explicit path with the original runtime UID and private permissions. Do not restore cache, tmp, run, or `ocd.lock` as authority. The OCD_DIR must already exist; `backup restore` creates only its missing scope lock.

For S3, restore the snapshot into the registered config's empty explicit `[data].path`, temporarily referencing the same operator-backed-up master key outside the target through that config. Afterward, put that key back under the instance data directory with mode `0600` and update its reference. For Local, `objects/` lives inside the data directory, so restore a **complete independently backed-up instance directory**, including `objects/format.json`, the key, and business data. `backup restore` does not support Local fresh-host recovery. Partial directories and Local↔S3 migration are not supported.

Read-only diagnosis for S3: install the snapshot's exact source release, check the registered config, key, and S3 authority, and confirm that the target instance data directory is missing or empty. This example selects a restored system scope and its default registered instance:

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

Allowed mutation:

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup restore --snapshot 0198f000-0000-7000-8000-000000000001 --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml doctor --full --json
```

A snapshot that includes Workflow must keep control/scheduler authority, restart/purge intent, operation progress, and GC receipts together. Original waiting/paused deadlines, inbox, and frozen retention are not recomputed. After restore, let the exact-release reconciler finish legal intermediate states, then verify original-version replay, paused state, events, and due work. Do not copy one database without the other, and do not delete operation rows to make diagnostics green.

Expect sibling staging, full verification, then one atomic install. Non-empty target, wrong key/release/object authority, path, hash, schema, or marker errors are stop conditions. Do not force or overwrite an old directory. On failure the target stays empty; the parent keeps a bounded `restore-failure` receipt and object staging under the same UUIDv7. After the diagnostic bytes are no longer needed, clean only the ID reported by that receipt:

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup cleanup-restore --staging 0198f000-0000-7000-8000-000000000002 --json
```

Rollback is: keep the target empty and fix key/release/object-authority/config. Cleanup refuses symlinks, hardlinks, non-regular files, non-manifest restore paths, and trees over the hard cap.

Verification: start the exact release, read KV/D1/DO/alarm sentinels, check deployment pins, basic WebSocket reconnect, a new write, and a second restart. After every step passes, stop the service again and record operator attestation. That command re-verifies snapshot, release, master key, platform identity, and the original restore receipt; it does not replace the product smoke above:

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup attest-restore-smoke --snapshot 0198f000-0000-7000-8000-000000000001 --passed --json
```

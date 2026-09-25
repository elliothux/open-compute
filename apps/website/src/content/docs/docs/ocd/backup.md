---
title: "Backup and retention"
---

Trigger: a planned maintenance window, a current-release restore drill, or an RPO deadline. Blast radius is local control / KV / D1 / DO / scheduler authority. R2 and other immutable references remain bound to the selected object authority; the snapshot authenticates those references but is not a second point-in-time copy of all object bytes. Runtime extraction cache is not snapshot authority.

With Local storage, a platform snapshot stored on the same disk is a consistency snapshot, not an off-host backup. The Local object root is always `<data.path>/objects`. To survive disk or host loss, stop `ocd` and independently back up the **complete instance data directory**, including `objects/format.json`, the actual `compute.toml`, and the master key file referenced by that config if it is outside the data directory. Local fresh-host recovery restores that complete directory and key; `backup restore` supports S3 snapshots only. There is no Local↔S3 migration or partial-directory restore.

Backup is offline for the selected instance: stop that instance (or the daemon), then take its data-dir lock. Restore requires the entire selected OCD daemon to be stopped and holds its scope lock; its target comes only from the selected `compute.toml` `[data].path` and may not overlap another registered instance data root. `dev` below is an example registered instance name; use `--system` as well for a system-scope installation. Back up its `compute.toml` separately when it is outside `[data].path`, and back up `OCD_DIR/ocd.toml`, global keys, and the complete `gateway/storage/` and `gateway/config-state/` directories together. Their storage identity markers must match on restart; missing ACME storage is not silently recreated. An instance snapshot does not include those files or other instances.

## Read-only diagnosis

```sh
ocd --instance dev doctor --json
ocd --instance dev backup list --json
```

## Create and verify

```sh
ocd --instance dev backup create --name nightly-20260826 --json
ocd --instance dev backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

`--name` is a bounded human audit label. `--snapshot` is a UUIDv7. `--verify` streams and hashes every owned object and immutable reference.

Expected output includes snapshot ID, exact bytes/files, and `verified=true`. Data-dir/object-root lock conflict, insufficient space, MAC/hash, authority marker, or immutable-reference failure are stop conditions.

`backup inspect` without `--verify` reads authenticated committed snapshot metadata only; it does not replace a full verify.

## Retention and delete

Only after **another verified snapshot already satisfies RPO** may you delete by exact ID:

```sh
ocd --instance dev backup delete --snapshot 0198f000-0000-7000-8000-000000000001 --json
```

Delete the manifest last. Rollback is: do not delete the old manifest.

Generate a delete plan without deleting objects:

```sh
ocd --instance dev backup retention-plan --keep-last 7 --json
```

Optional `--max-age-seconds` and repeatable `--keep-label`. After reviewing the plan, `backup delete` each listed ID. Do not remove Local envelope files or issue your own S3 bulk delete against the snapshot prefix.

Incomplete uploads older than the configured grace:

```sh
ocd --instance dev backup cleanup-incomplete --json
```

## Verification

Re-run `backup list` / `backup inspect --verify`, and confirm doctor reads `last-snapshot.json`. Do not record a verification that was not actually executed.

Restore steps are in the [incident handbook](/docs/ocd/incidents/): current-release restore and fresh-host restore. Restore does not undo external side effects after the snapshot (including the current R2 state).

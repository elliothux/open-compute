# Backup and retention

Trigger: a planned maintenance window, a current-release restore drill, or an RPO deadline. Blast radius is local control / KV / D1 / DO / scheduler authority. R2 is bound to the current external S3; it is **not** a point-in-time copy (`OC-R2-001`). Runtime extraction cache is not snapshot authority.

Backup and restore are offline: stop the service, then take the data-dir lock.

## Read-only diagnosis

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --json
/opt/open-compute/platformd --config /etc/open-compute/config.toml backup list --json
```

## Create and verify

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml backup create --name nightly-20260826 --json
/opt/open-compute/platformd --config /etc/open-compute/config.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

`--name` is a bounded human audit label. `--snapshot` is a UUIDv7. `--verify` streams and hashes every owned object and immutable reference.

Expected output includes snapshot ID, exact bytes/files, and `verified=true`. Data-dir lock conflict, insufficient space, MAC/hash, bucket marker, or immutable-reference failure are stop conditions.

`backup inspect` without `--verify` reads authenticated committed snapshot metadata only; it does not replace a full verify.

## Retention and delete

Only after **another verified snapshot already satisfies RPO** may you delete by exact ID:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml backup delete --snapshot 0198f000-0000-7000-8000-000000000001 --json
```

Delete the manifest last. Rollback is: do not delete the old manifest.

Generate a delete plan without deleting objects:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml backup retention-plan --keep-last 7 --json
```

Optional `--max-age-seconds` and repeatable `--keep-label`. After reviewing the plan, `backup delete` each listed ID. Do not write your own S3 bulk delete against the snapshot prefix.

Incomplete uploads older than the configured grace:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml backup cleanup-incomplete --json
```

## Verification

Re-run `backup list` / `backup inspect --verify`, and confirm doctor reads `last-snapshot.json`. Do not record a verification that was not actually executed.

Restore steps are in the [incident handbook](/en/incidents/): current-release restore and fresh-host restore. Restore does not undo external side effects after the snapshot (including the current R2 state).

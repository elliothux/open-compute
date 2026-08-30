# Current-release restore

Trigger: current binary is corrupt, host recovery, or schema/runtime identity checks fail. Blast radius is the platform data directory, the current executable/runtime pin, and all authority in the snapshot. There is no product upgrade path across historical development versions.

Read-only diagnosis: compare the binary capability's full release identity with the current config; do not modify existing databases:

```sh
/opt/open-compute/platformd capabilities --json
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --json
```

Allowed mutation: after operator confirmation, stop the service, use a verified binary of the **same** release, and restore an authenticated snapshot into an explicitly new directory using [fresh-host restore](/en/incidents/fresh-host). Source release, config policy, master key, S3 authority, and full schema identity must match first. Do not overwrite, downgrade, self-repair, or empty the existing directory. Building a release, downloading a runtime, and replacing the binary still need separate approval.

Expect the current schema to be strictly verified. Restore publishes only a complete new directory and writes `last-restore.json`. Unknown or mismatched schema, checksum, release/pin, config policy, snapshot signature, or key are stop conditions. There is no upgrade command that bypasses these checks.

Rolling back current data state likewise uses a verified snapshot of the same release into a brand-new directory, accepting that snapshot's RPO. Restore does not undo external side effects after the snapshot. Worker deployment promote/rollback is a separate supported product operation; it is not a platform history upgrade.

Verification: read-only doctor, current product smoke, write/read after restart, and snapshot inspect. Do not record a verification that was not actually executed.

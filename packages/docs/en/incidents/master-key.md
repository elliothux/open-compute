# Master-key loss

Trigger: `master_key_mismatch`, missing key file, or secret decrypt canary failure. Blast radius is every encrypted secret in control, snapshot MACs, and disaster recovery.

Read-only diagnosis: stop the service and check that the recovery key fingerprint matches snapshot/control identity:

```sh
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml doctor --json
/opt/open-compute/platformd --config /etc/open-compute/recovery.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

Allowed mutation: place the **same** operator-backed-up key at `/etc/open-compute/recovery-master.key` with mode 0600, then [fresh-host restore](/en/incidents/fresh-host). Expect fingerprint, decrypt canary, and manifest MAC to match together. Do not generate a new key over the old platform, and do not write the key into a data-dir snapshot. Stop condition: no matching key — not recoverable. Rollback is removing the wrong key reference and keeping evidence. Verification: decrypt canary, manifest MAC, doctor full, tenant secret binding, and restart.

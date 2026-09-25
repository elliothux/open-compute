---
title: "Master-key loss"
---

Trigger: `master_key_mismatch`, missing key file, or secret decrypt canary failure. Blast radius is every encrypted secret in control, snapshot MACs, and disaster recovery.

Read-only diagnosis: stop the selected scope's daemon, obtain the **same** key from an independent operator backup, and temporarily reference it from the registered `compute.toml` to check its fingerprint against snapshot/control identity. This system-scope example selects the default instance:

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml doctor --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

Allowed mutation: restore that **same** key with mode 0600 to the file referenced by the instance config, or verify through a temporary external reference and then put it back under the instance data directory. Do not generate a new key over existing authority. If the data directory is also lost, follow the S3/Local branches of [fresh-host restore](/docs/ocd/incidents/fresh-host/). Expect fingerprint, decrypt canary, and manifest MAC to match together; never put the key in a snapshot. No matching key means recovery stops. Roll back a wrong reference while preserving evidence. Verify with doctor full, tenant secret binding, and restart.

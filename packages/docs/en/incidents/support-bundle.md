# Collect a support bundle

Trigger: need an offline diagnosis of release, doctor, metrics, schema, or recent receipts. Blast radius is one new bounded local tar; nothing is uploaded automatically.

Read-only diagnosis: confirm the output parent exists, the target file does not exist, and it is not a symlink:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --json
```

Allowed mutation:

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml support-bundle --output /var/tmp/open-compute-support-20260826.tar --json
/usr/bin/shasum -a 256 /var/tmp/open-compute-support-20260826.tar
```

Expect archive mode 0600, containing only allowlisted release, redacted policy, doctor, metrics, schema, bounded events/receipts, and file digests. Secret canary, size cap, target already exists, or a non-canonical path are stop conditions. Do not bypass the scanner or hand-add DB, DO, bundle, request body, key, or credential. Rollback is a controlled destroy of that exact file by the operator. Verification: check the output SHA-256, inspect entry names offline, and confirm no secrets.

# Install and first start

Trigger: a new host has not generated platform identity, or readiness has never succeeded. Blast radius is the entire single-node platform.

## Install this one file

Install only the OS/CPU-matching `ocd` file at `/opt/open-compute/ocd` and verify the publisher SHA-256. There is no adjacent workerd, runtime, or share directory requirement. Runtime does not install or download any tools, and you must not place a separate workerd next to the binary.

The artifact is a single file, but the first run extracts and verifies the embedded runtime under the data-dir (`data/runtime/packages/<payload-sha256>/`). That is not "the disk will ever contain only this file". The filesystems that hold the data-dir and (on macOS) staging must be executable. Official Linux workerd needs glibc 2.35+; the container example uses Ubuntu 24.04, not scratch/Alpine.

## Config and accounts

Read-only prep: `ocd config init` writes a template to stdout and does not initialize files or secrets:

```sh
/opt/open-compute/ocd config init --data-dir /var/lib/open-compute > /etc/open-compute/config.toml
```

Save it as a **new** `/etc/open-compute/config.toml` (do not overwrite an existing file). Set the S3 endpoint, bucket, and env/file credential references. Config, credentials, and the data directory are owned by a dedicated service account.

This site, the README, the install runbook, and the systemd/container examples use `/etc/open-compute/config.toml`. `--config` must be an absolute path; it is never searched from cwd or `$HOME`. Some embedded runbooks still show `/etc/open-compute/platform.toml` as an example filename; that is not a second config format. The macOS launchd example uses `/usr/local/etc/open-compute/config.toml` and `/usr/local/var/open-compute`.

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml config check --json
/opt/open-compute/ocd capabilities --json
```

`--help`, `--version`, `capabilities`, `docs`, `licenses`, and `config init/check` do not materialize the runtime.

## First `run`

Allowed mutation: provision the dedicated account and a writable data-dir, configure S3 authority, then:

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml run
```

After taking the exclusive data-dir lock, first start generates platform identity, databases, and the master key, extracts and verifies the embedded runtime offline, then checks S3, compiles the system config, and starts workerd. Expect both `/health/live` and `/health/ready` to succeed.

Do not start a second `ocd` on the same data-dir (`DATA_DIR_IN_USE`).

## When `doctor --full` is allowed

Plain `doctor` does not initialize the directory. Full diagnostics that need existing data and identity run only after the **first successful run and a clean shutdown**: they take the exclusive data-dir lock and perform an S3 canary / temporary runtime.

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --full --json
```

Do not require `doctor --full` to succeed before first initialization.

## Stop conditions and rollback

Stop conditions: master key, S3 authority, runtime digest, permission, or free-space checks fail. Do not regenerate keys in a loop, and do not bypass errors with an external workerd or a re-download. Rollback is: stop the process and keep config, key, and data-dir.

Verification: one smoke Worker request, a read after restart, and a full doctor after shutdown.

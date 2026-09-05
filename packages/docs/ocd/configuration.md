# Configuration

`--config` names one exact regular file. A relative value is resolved once against the startup working directory; an absolute value keeps its absolute meaning. The file leaf is opened without following symlinks, and no parent or `$HOME` search occurs. Relative filesystem paths inside TOML are resolved against the canonical directory containing the opened config. `.` and `..` are normalized; `~`, environment references, globs, and URIs are not expanded. Parse time does not read `.env` or resolve secret values. Unknown fields are rejected.

Path examples on this page use `/etc/open-compute/config.toml`. Some embedded runbooks write `platform.toml`; the flag is only `--config`, not a second format keyed by filename.

```sh
ocd config init --data-dir /var/lib/open-compute > /etc/open-compute/config.toml
ocd --config /etc/open-compute/config.toml config check
```

`config init` resolves `data-dir` against the startup working directory, writes absolute paths into the template, and prints it to stdout. It does not create directories or write secrets. `config check` is static parse and validation only.

The embedded default template matches `share/default-config.toml`. Live numeric limits come from `ocd --config /abs/config.toml capabilities --json` `limits`.

## Secrets

Secrets are references only. Do not put them in units, images, the repository, or config plaintext.

- `server.admin_auth`, `server.deployer_auth`, and `server.read_only_auth`: three required, mutually distinct Bearer token references using `env` and/or a `file` path.
- S3 backend only: `storage.access_key_id_env` / `storage.access_key_id_file` and `storage.secret_access_key_env` / `storage.secret_access_key_file`; each pair needs at least one. Local never reads these variables.
- Master key: `data.master_key_file`; optional `data.master_key_env`.
- Environment variable names must be non-empty ASCII uppercase, digits, and underscore, and must not start with a digit.
- Tenant binding names must not start with `OPEN_COMPUTE_`; that prefix is reserved by the platform, not a license to inline secrets.

Every admin listener, including loopback, requires all three role tokens. Startup rejects equal resolved token values instead of relying on match order.

## `[data]`: platform state and lock

`[data]`:

| Field | Role |
| --- | --- |
| `path` | Data root. SQLite, identity, master key, runtime extraction, and cache live here |
| `master_key_file` | Master key path |
| `sqlite_busy_timeout_ms` | SQLite `busy_timeout` |
| `free_space_soft_bytes` | Health degrades below this |
| `free_space_hard_bytes` | Mutations refused below this; must be ≤ soft |

One `ocd` per data-dir. The exclusive lock is `<data_dir>/platform.lock`. A second instance gets `DATA_DIR_IN_USE`; do not bypass it. The data-dir must be writable and executable (the extracted workerd runs from here).

## `[storage]`: object bytes

`storage.backend` is required and is exactly `local` or `s3`. The variants are mutually exclusive, with no fallback, dual write, or automatic migration. Both use disjoint canonical `prefix` / `r2_prefix` values.

Local fields:

| Field | Constraint |
| --- | --- |
| `path` | Secure local object root; either `<data.path>/objects` or disjoint from `data.path` |
| `free_space_soft_bytes` | Object-storage health degrades below this |
| `free_space_hard_bytes` | Object writes are refused below this; must be ≤ soft |
| `partial_grace_ms` | Minimum age before strictly owned crash remnants are reclaimed |

The local root must be a mode-0700 directory on a supported local filesystem. Symlinks, special files, unexpected entries, insecure modes, and network/FUSE filesystems fail closed. Local is direct filesystem storage; it does not start an S3 server or rclone.

S3 uses AWS SDK SigV4:

| Field | Constraint |
| --- | --- |
| `endpoint` | Service URL |
| `region` | Non-empty; `auto` is accepted |
| `bucket` | Non-empty |
| `force_path_style` | Default `true` |
| `verify_tls` | Cannot be disabled |
| `prefix` / `r2_prefix` | Must be canonical and disjoint |

A failed upload is not committed. An initialized platform is bound to its backend kind and authority fingerprint. Do not temporarily switch backend, root, provider, bucket, or prefix to "just get it running".

## Other sections

The template also includes `[server]`, `[runtime]`, `[cache]`, `[response_cache]`, `[images]`, `[metrics]`, `[hardening]`, `[workers]`, `[kv]`, `[r2]`, `[d1]`, `[queues]`, `[durable_objects]`, `[scheduler]` (including pools), and `[workflows]`. These are local quotas and timeouts, not Cloudflare plan SKUs. Run `config check` before changing them, then `capabilities --json` for actual `limits`.

`hardening.emergency_reserve_bytes` must be below the `[data]` hard reserve.

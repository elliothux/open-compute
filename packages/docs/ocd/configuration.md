# Configuration

`--config` must be an absolute regular file. It is not searched from cwd or `$HOME`, symlinks are not followed, and the path must not contain `..`. Parse time does not read `.env` and does not resolve secret values. Unknown fields are rejected. `runtime.binary`, `runtime.lock_file`, and `runtime.assets_dir` are unknown keys and are refused before startup.

Path examples on this page use `/etc/open-compute/config.toml`. Some embedded runbooks write `platform.toml`; the flag is only `--config`, not a second format keyed by filename.

```sh
ocd config init --data-dir /var/lib/open-compute > /etc/open-compute/config.toml
ocd --config /etc/open-compute/config.toml config check
```

`config init` writes the given absolute `data-dir` into the template and prints it to stdout. It does not create directories or write secrets. `config check` is static parse and validation only.

The embedded default template matches `share/default-config.toml`. Live numeric limits come from `ocd --config /abs/config.toml capabilities --json` `limits`.

## Secrets

Secrets are references only. Do not put them in units, images, the repository, or config plaintext.

- `server.admin_auth`: `env` and/or an absolute `file` (the TOML secret object).
- S3: `access_key_id_env` / `access_key_id_file` and `secret_access_key_env` / `secret_access_key_file`; each pair needs at least one.
- Master key: `storage.master_key_file` (absolute); optional `storage.master_key_env`.
- Environment variable names must be non-empty ASCII uppercase, digits, and underscore, and must not start with a digit.
- Tenant binding names must not start with `OPEN_COMPUTE_`; that prefix is reserved by the platform, not a license to inline secrets.

A non-loopback admin listener requires `server.admin_auth`.

## Data-dir and lock

`[storage]`:

| Field | Role |
| --- | --- |
| `data_dir` | Absolute data root. SQLite, identity, master key, runtime extraction, and cache live here |
| `master_key_file` | Absolute master key path |
| `sqlite_busy_timeout_ms` | SQLite `busy_timeout` |
| `free_space_soft_bytes` | Health degrades below this |
| `free_space_hard_bytes` | Mutations refused below this; must be ≤ soft |

One `ocd` per data-dir. The exclusive lock is `<data_dir>/platform.lock`. A second instance gets `DATA_DIR_IN_USE`; do not bypass it. The data-dir must be writable and executable (the extracted workerd runs from here).

## S3 (SigV4)

S3 is part of platform authority; the single file does not embed object storage. The protocol is AWS SDK SigV4.

| Field | Constraint |
| --- | --- |
| `endpoint` | Service URL |
| `region` | Non-empty; `auto` is accepted |
| `bucket` | Non-empty |
| `force_path_style` | Default `true` |
| `verify_tls` | Cannot be disabled |
| `prefix` / `r2_prefix` | Must be disjoint; `prefix` must not start with `tenant/` |

A failed upload is not committed. Do not temporarily switch provider/prefix to "just get it running".

## Other sections

The template also includes `[server]`, `[runtime]`, `[cache]`, `[response_cache]`, `[images]`, `[metrics]`, `[hardening]`, `[workers]`, `[kv]`, `[r2]`, `[d1]`, `[queues]`, `[durable_objects]`, `[scheduler]` (including pools), and `[workflows]`. These are local quotas and timeouts, not Cloudflare plan SKUs. Run `config check` before changing them, then `capabilities --json` for actual `limits`.

`hardening.emergency_reserve_bytes` must be below the storage hard reserve.

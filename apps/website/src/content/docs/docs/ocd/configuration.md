---
title: "Configuration"
---

`--config` names one exact regular file. A relative value is resolved once against the startup working directory; an absolute value keeps its absolute meaning. The file leaf is opened without following symlinks, and no parent or `$HOME` search occurs. Relative filesystem paths inside TOML are resolved against the canonical directory containing the opened config. `.` and `..` are normalized; `~`, environment references, globs, and URIs are not expanded. Parse time does not read `.env` or resolve secret values. Unknown fields are rejected.

## Configuration ownership

For a user-facing map of how the daemon, instances, data directories, Gateway, and extensions fit together, see [Architecture and boundaries](/docs/ocd/architecture/).

There are two configuration layers:

- `<OCD_DIR>/ocd.toml` configures the daemon scope: shared listeners, the admin credential, Gateway, global limits, and the explicit instance registry.
- Each registered `compute.toml` configures one isolated instance: its data and object authority, credentials, products, Dashboard, public base domain, and extensions.

```toml title="ocd.toml"
[server]
public_bind = "127.0.0.1:8787"
admin_auth = { file = "./keys/admin.token" }

[artifacts]
max_concurrent_requests = 16

[metrics]
max_series = 1024

[[instances]]
config = "instances/default/compute.toml"
autostart = true
```

Managed instances are listed explicitly in `<OCD_DIR>/ocd.toml`. Its `[server]` section owns the shared `public_bind` and optional `admin_bind`; `[artifacts].max_concurrent_requests` (default 16) bounds Git requests across all instances, and `[metrics].max_series` (default 1024) bounds the daemon's exposed metric series. Each `[[instances]]` entry contains only `config` and `autostart`; identity, data paths, digests, and process state are not copied into the manifest. The user OCD directory is the running UID's account home plus `.open-compute`, independent of an overridden `HOME`; the system OCD directory is `/var/lib/open-compute`.

`<OCD_DIR>/instances/` is only the setup default location. It is never scanned, does not register or start an instance, and does not derive identity or data location. External configuration and data paths remain supported.

Each enabled instance currently exposes 752 fixed metric series. The shared default of 1024 admits one complete instance scrape; another new scrape gets `503` without stopping either instance. Raise `[metrics].max_series` in `ocd.toml` for multiple concurrent metric targets. Stopping an instance releases its registered series.

Registered instances cannot claim the same public base domain or parent/child domains. Registration and daemon startup reject those conflicts before selecting any winner.

Path examples on this page use the system default instance. Some embedded runbooks use other exact filenames; the flag is only `--config`, not a second format keyed by filename.

```sh
ocd config init --data-dir /var/lib/open-compute/instances/default/data > /var/lib/open-compute/instances/default/compute.toml
ocd --config /var/lib/open-compute/instances/default/compute.toml config check
```

`config init` resolves `data-dir` against the startup working directory, writes absolute paths into the template, and prints it to stdout. It does not create directories or write secrets. `config check` is static parse and validation only.

The embedded default template matches `share/default-config.toml`. Live numeric limits come from `ocd --config /abs/config.toml capabilities --json` `limits`.

## Operator HTTP proxy

Operator-owned AI, target, release, and remote S3 requests select one proxy when `ocd` starts. The first non-empty variable wins:

```text
HTTPS_PROXY → https_proxy → ALL_PROXY → all_proxy → HTTP_PROXY → http_proxy → direct
```

The selected value must be a credential-free canonical `http://host:port` URL. `NO_PROXY` takes precedence over `no_proxy` and accepts `*`, exact IPs, IP CIDRs, exact domains, and domain suffixes. Loopback destinations always connect directly. macOS System Settings, PAC/WPAD, SOCKS, proxy authentication, interception CAs, and OS proxy discovery are not supported. Put the variables in the actual shell, launchd unit, or service environment that starts `ocd`; invalid or unreachable explicit proxies fail closed. Tenant Worker egress and public Git imports do not use this policy.

`GET /client/v4/open-compute/system/status` reports only `direct`, `proxy`, or `invalid`; for a valid proxy it also reports the selecting variable and credential-free origin.

## Secrets

Secrets are references only. Do not put them in units, images, the repository, or config plaintext.

- `server.admin_auth` in the scoped `ocd.toml` is the single global admin Bearer token reference. Each `compute.toml` has only `auth.deployer_auth` and `auth.read_only_auth`; their tokens must be distinct from each other and the global admin token. References use `env` and/or a `file` path.
- S3 backend only: `storage.access_key_id_env` / `storage.access_key_id_file` and `storage.secret_access_key_env` / `storage.secret_access_key_file`; each pair needs at least one. Local never reads these variables.
- Master key: `data.master_key_file`; optional `data.master_key_env`.
- Environment variable names must be non-empty ASCII uppercase, digits, and underscore, and must not start with a digit.
- Tenant binding names must not start with `OPEN_COMPUTE_`; that prefix is reserved by the platform, not a license to inline secrets.

Every admin listener, including loopback, requires all three role tokens. Startup rejects equal resolved token values instead of relying on match order.

## Instance identity and optional surfaces

`[instance].name` is the mutable CLI and Dashboard display name. The durable InstanceId is initialized in the instance data authority and is not configurable.

Enable the operator UI in the instance configuration:

```toml
[dashboard]
enabled = true
```

An instance may also claim one public Gateway domain:

```toml
[public_gateway]
base_domain = "compute.example.com"
```

See [Dashboard](/docs/ocd/dashboard/) and [Gateway](/docs/gateway/). Shared Gateway listeners and Caddy inputs remain in `ocd.toml`.

## `[ai]`: provider backends and embedding profiles

An AI backend is one operation-specific, final request URL. `ocd` never appends `/embeddings` or `/chat/completions`, so include any provider path prefix and the operation route in `endpoint`:

```toml
[ai]
default_embedding_model = "company/qwen-embedding"

[ai.backends.bailian-embeddings]
protocol = "openai_embeddings_v1"
endpoint = "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings"
auth = { kind = "bearer", secret = { env = "DASHSCOPE_API_KEY" } }
headers = { "X-Title" = "open-compute" }

[ai.embedding_profiles."qwen/qwen3-1024"]
dimensions = 1024
max_input_tokens = 8192
send_dimensions = true
tokenizer = { kind = "qwen3", revision = "pinned-tokenizer-revision", artifact = { path = "/opt/open-compute/tokenizers/qwen3/tokenizer.json", sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" } }

[ai.embedding_models."company/qwen-embedding"]
backend = "bailian-embeddings"
remote_model = "text-embedding-v4"
profile = "qwen/qwen3-1024"
```

`default_embedding_model` is required before AI Search instances can be created. Add `default_generation_model` and a matching `generation_models` entry only when AI Search chat, query rewrite, or reranking is needed.

Authentication is a closed choice: `bearer`, one custom secret `header`, or `none`. For providers that require a custom key header, use `auth = { kind = "header", name = "X-API-Key", secret = { file = "/run/secrets/provider-key" } }`. The optional `headers` map is only for non-secret static metadata. It cannot override `Authorization`, the custom auth header, host/content headers, cookies, proxy headers, or hop-by-hop headers. `none` is accepted only for loopback HTTP; non-loopback endpoints require HTTPS.

Profiles keep model facts reusable without making them implicit. Dimensions, maximum input tokens, whether to send `dimensions`, and a digest-pinned offline tokenizer belong in the profile. AI Search fixes its metric to cosine. `config check` validates the artifact declaration without reading it; `ocd` verifies the local bytes while composing AI Search services and never downloads a tokenizer.

## `[data]`: platform state and lock

`[data]` is required, and `path` is required inside it. The runtime never infers a data directory from the config filename, its parent, or an `instances/` directory:

| Field                    | Role                                                                                       |
| ------------------------ | ------------------------------------------------------------------------------------------ |
| `path`                   | Data root. SQLite, identity, local objects, per-instance runtime state and cache live here |
| `master_key_file`        | Master key path; resolved from this configuration and may be outside the data root         |
| `sqlite_busy_timeout_ms` | SQLite `busy_timeout`                                                                      |
| `free_space_soft_bytes`  | Health degrades below this                                                                 |
| `free_space_hard_bytes`  | Mutations refused below this; must be ≤ soft                                               |

Relative `data.path` values resolve against the directory containing `compute.toml`. The shared verified runtime package lives under `<OCD_DIR>/cache/packages/`; only per-instance runtime config, lease, and staging state live under this data root. If the resolved data root is inside OCD_DIR, it must be a strict descendant of `<OCD_DIR>/instances/`; OCD_DIR itself, the `instances/` container, `instances-old/`, and every other OCD subtree are rejected. An external data root is allowed, but it cannot contain OCD_DIR. Registered data roots cannot overlap.

One scoped `ocd` daemon owns the registered instances. Each instance has an exclusive `<data.path>/platform.lock`; do not bypass it. The data directory must be writable and executable.

## `[storage]`: object bytes

`storage.backend` is required and is exactly `local` or `s3`. The variants are mutually exclusive, with no fallback, dual write, or automatic migration. Both use disjoint canonical `prefix` / `r2_prefix` values.

Local fields:

| Field                   | Constraint                                                     |
| ----------------------- | -------------------------------------------------------------- |
| `free_space_soft_bytes` | Object-storage health degrades below this                      |
| `free_space_hard_bytes` | Object writes are refused below this; must be ≤ soft           |
| `partial_grace_ms`      | Minimum age before strictly owned crash remnants are reclaimed |

The local root is always `<data.path>/objects`; `storage.path` is not accepted. It must be a mode-0700 directory on a supported local filesystem. Symlinks, special files, unexpected entries, insecure modes, and network/FUSE filesystems fail closed. Local is direct filesystem storage; it does not start an S3 server or rclone.

S3 uses AWS SDK SigV4:

| Field                  | Constraint                     |
| ---------------------- | ------------------------------ |
| `endpoint`             | Service URL                    |
| `region`               | Non-empty; `auto` is accepted  |
| `bucket`               | Non-empty                      |
| `force_path_style`     | Default `true`                 |
| `verify_tls`           | Cannot be disabled             |
| `prefix` / `r2_prefix` | Must be canonical and disjoint |

Instances sharing one S3 endpoint and bucket must configure separate, non-overlapping `prefix` and `r2_prefix` values; the defaults cannot be reused for both. Registration rejects overlaps across either prefix, and startup binds both remote prefix markers to the instance ID. Missing or mismatched markers fail closed.

A failed upload is not committed. An initialized platform is bound to its backend kind and authority fingerprint. Do not temporarily switch backend, root, provider, bucket, or prefix to "just get it running".

## `[extensions.<name>]`: trusted local native extensions

On macOS and Linux, an operator may statically expose a local extension as a Service Binding target:

```toml
[extensions.local-files]
path = "./extensions/local-files"
```

The path is resolved relative to the loaded instance config. The directory must contain strict `extension.toml` entries for one bundled facade module and one executable Provider. Extensions are trusted operator code, load only when that instance starts, receive no tenant secrets or platform credentials, and are not installed, downloaded, versioned, hot-reloaded, or sandboxed by `ocd`. Their names share the instance's Worker service namespace and may not collide with a live Worker. See [Extensions](/docs/extension/).

## `[private_services.<name>]`: fixed private HTTP Service targets

An operator may expose one fixed private or loopback HTTP endpoint through the standard Service Binding `fetch()` interface without opening general tenant egress to private addresses:

```toml
[private_services.inventory]
scheme = "http"
host = "10.20.0.15"
port = 8080
path_prefixes = ["/v1/"]
methods = ["GET", "POST"]
credential_header = "x-api-key"
credential = { file = "/run/secrets/inventory-key" }
allow = [{ account_id = "<instance-id>", worker_id = "<worker-id>", entrypoint = "api" }]
```

The endpoint is resolved to a private address and pinned when `ocd` starts. Redirects are returned, never followed. The caller cannot choose the URL or credential; internal and tenant authentication headers are stripped, and the configured credential is injected only on the host-side request. Upload admission and every invocation recheck the exact instance, Worker, optional Version, entrypoint, and policy revision. A newly created Worker therefore needs its stable Worker ID before this binding can be added. Private targets support Service Binding HTTP `fetch()` only; RPC and `connect()` fail closed. Ordinary tenant `fetch()` remains public-address-only.

## Other sections

The instance template also includes `[instance]`, `[auth]`, `[runtime]`, `[cache]`, `[response_cache]`, `[images]`, `[ai]`, `[document_parser]`, `[observability]`, `[metrics]`, `[hardening]`, `[workers]`, `[kv]`, `[r2]`, `[d1]`, `[queues]`, `[durable_objects]`, `[scheduler]` (including pools), `[workflows]`, optional `[dashboard]`, `[public_gateway]`, `[extensions.<name>]`, and `[private_services.<name>]`. Public listener settings belong only in `ocd.toml`, not `compute.toml`. These are local quotas and timeouts, not Cloudflare plan SKUs. Run `config check` before changing them, then `capabilities --json` for actual `limits`.

`hardening.emergency_reserve_bytes` must be below the `[data]` hard reserve.

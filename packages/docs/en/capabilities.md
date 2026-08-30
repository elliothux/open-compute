# Capabilities and limits

Trust the binary on this machine. Do not infer full Cloudflare behavior from product names. Cloudflare features that do not appear in `capabilities --json` are unsupported. This page is not a complete Cloudflare matrix.

```sh
platformd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. Without it, `limits` come from the embedded default config; with an absolute config path, `limits` reflect that file. Top-level JSON fields:

## schema_version

Always `1`. Do not treat any other value as this contract.

## release

Exact release identity, not a marketing version. It includes `platform_version`, `git_revision`, `rust_msrv`, `workerd_version`, `workerd_lock_sha256`, `runtime_assets_sha256`, `facade_capability_version`, control / scheduler / KV / D1 schema versions, `snapshot_format_version`, and `compatibility_policy_sha256`. When restoring or replacing a binary, match this identity against the snapshot and schema, not the filename.

## runtime

Pinned workerd compatibility policy:

| Field | Meaning |
| --- | --- |
| `compatibility_date_min` / `compatibility_date_max` | Inclusive Worker compatibility-date range |
| `allowed_flags` | Allowlisted compatibility flags |
| `denied_flags` | Explicitly denied flags |
| `workerd_lock_sha256` | SHA-256 of the formal runtime lock bytes |

Do not swap a workerd beside the binary, search `PATH`, or download another runtime. A digest mismatch is a stop condition.

## products

Keyed by stable product name. Each entry has:

| Field | Meaning |
| --- | --- |
| `status` | `supported` / `unsupported` / `conditional` |
| `capability_version` | Present for `supported` facade version; omitted for `unsupported` / `conditional` |
| `methods` | Supported method names in canonical order |
| `deviations` | Registered deviation IDs |
| `basic_websocket` | Optional on Durable Objects; basic WebSocket state |
| `hibernatable_websocket` | Optional on Durable Objects; hibernatable WebSocket state |

`supported`: production behavior and its Gate are implemented. `unsupported`: intentionally absent. `conditional`: a pinned-runtime hard Gate has not produced a stable Go. Unlisted products or APIs are not supported.

Registered product names include `workers`, `kv`, `r2`, `d1`, `durable_objects`, `alarms`, `queues`, `cron`, `workflows`, `workers_cache`, `cache_api`, `images`, `version_metadata`, and `websocket_hibernation`. Durable Objects also report `basic_websocket` (supported) and `hibernatable_websocket` (unsupported). The `websocket_hibernation` product itself is `unsupported`.

## limits

Frozen numeric ceilings from config. **No secrets.** Exact numbers come from the current `capabilities --json`; do not copy this page or the default TOML as live quotas.

## Registered deviations

These IDs appear in the matching product `deviations`. Their meaning is the registry; a missing ID does not imply other Cloudflare behavior is available.

### OC-KV-001

KV is single-node SQLite authority; it does not claim Cloudflare global replication or propagation timing.

### OC-R2-001

R2 is backed by the configured S3 authority. A full platform snapshot records bucket identity but does not provide R2 point-in-time recovery.

### OC-D1-001

D1 session constraints and bookmark replication are not implemented; `withSession()` only exposes the explicitly documented local behavior.

### OC-DO-001

Durable Objects are placed on the single local workerd process; placement hints and global migration are unsupported.

### OC-WS-001

Basic Durable Object WebSocket is supported. Native hibernatable WebSocket remains disabled until the pinned stock-workerd hard Gate is a complete Go.

### OC-QUEUE-001

Queue producers and push consumers are backed by single-node `scheduler.sqlite` durability, not Cloudflare global replication. Delivery is at-least-once without strict FIFO. An unknown native dispatch retains its lease and does not consume the tenant retry budget, so a later delivery can repeat the same attempt number. JSON is the default for every supported compatibility date; `v8`, metadata, pull consumers, multiple consumers per Queue, resource-level PITR, and Cloudflare plan quotas are unsupported. Durable Object Queue writes remain fail closed with `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED` because the service-facade transport cannot inherit stock workerd's native Durable Object output gate.

### OC-CRON-001

Cron is UTC-only with five fields and the documented local Quartz-like extensions. Recovery projects at most the newest slot within the configured misfire grace rather than replaying complete downtime history. Known failures use the configured bounded local retry policy unless `noRetry()` is called.

### OC-WORKFLOW-001

Workflow uses local SQLite authority and bounded canonical JSON rather than full structured clone. The current model supports retries, attempt timeouts, durable sleep and event waiting, lifecycle modifiers, frozen retention, and bounded synchronous `step.do` batches. Parallel waits, arbitrary Promise graphs, batch instance creation, dynamic retry functions, rollback hooks, restart-from-step, full structured clone, and exactly-once external effects are unsupported. Callbacks are at-least-once until their result commits; replay skips durably completed callbacks, and external product effects do not roll back with Workflow snapshots.

### OC-WORKFLOW-002

Durable Object Workflow mutations (`create`, `sendEvent`, `pause`, `resume`, `terminate`, `restart`) fail closed with `WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED`: the independent pinned-workerd probe cannot prove that this service-facade transport inherits the native Durable Object output gate. Read-only `get` and `status` remain available. In the other direction, a Workflow calling a DO still follows the existing active Worker deployment check; a retired deployment receives `DO_DEPLOYMENT_STALE`.

### OC-CACHE-001

Workers Cache and Cache API are single-node local authority. Automatic caching requires an explicit `s-maxage` or `max-age`; heuristic TTL, global replication/purge propagation, tiered cache, Cache Rules, Cache Deception Armor, and plan-dependent behavior are unsupported.

### OC-CACHE-002

The operator-configured default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. The exact active values are emitted by `platformd capabilities --json`.

### OC-IMAGES-001

Images is a bounded local raster transform binding, not hosted Cloudflare Images. Day1 input is JPEG/PNG/WebP; output is JPEG/PNG/WebP/AVIF; animated inputs, arbitrary ICC preservation, SVG, hosted delivery/upload/signing, URL transforms, video, AI upscale, and `fetch(..., {cf:{image}})` are unsupported.

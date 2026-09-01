# Capabilities and limits

Trust the binary on this machine. Do not infer full Cloudflare behavior from product names. Cloudflare features that do not appear in `capabilities --json` are unsupported. This page is not a complete Cloudflare matrix.

```sh
platformd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. Without it, `limits` come from the embedded default config; with an absolute config path, `limits` reflect that file. Top-level JSON fields:

## schema_version

Always `1`. Do not treat any other value as this contract.

## release

Exact release identity, not a marketing version. It includes `platform_version`, `git_revision`, `rust_msrv`, `workerd_version`, `workerd_lock_sha256`, `runtime_assets_sha256`, `facade_capability_version`, control / scheduler / KV / D1 schema versions, and `snapshot_format_version`. When restoring or replacing a binary, match this identity against the snapshot and schema, not the filename.

## runtime

Pinned workerd baseline:

| Field | Meaning |
| --- | --- |
| `effective_compatibility_date` | The one effective compatibility date from the formal runtime lock |
| `workerd_lock_sha256` | SHA-256 of the formal runtime lock bytes |
| `workers_types_version` | Pinned `@cloudflare/workers-types` version |
| `workers_types_git_head` | Upstream git revision for that types package |
| `workers_types_package_sha256` | Types package digest |
| `workers_types_index_sha256` | SHA-256 of the pinned stable `index.d.ts` bytes |
| `workers_types_ast_sha256` | SHA-256 of the canonical AST of the pinned stable declaration |

Do not swap a workerd beside the binary, search `PATH`, or download another runtime. A digest mismatch is a stop condition.

## products

Keyed by stable product name. Each entry has:

| Field | Meaning |
| --- | --- |
| `status` | `supported` / `supported_with_deviation` / `blocked` / `unsupported` |
| `kind` | `target` (upstream AST inventory), `platform` (platform-owned), or `non_target` (explicitly out of scope) |
| `capability_version` | Static facade version when the product is fully supported; omitted for `blocked` / `unsupported` |
| `members` | Per-member/overload records for target products. Each record has a stable id, symbol, member, kind, overload, readonly/optional/static, signature, `signature_sha256`, status, and evidence cases |
| `deviations` | Registered single-machine topology or stock-runtime capacity deviation IDs |

`supported`: every target member has compile and real-runtime evidence. `supported_with_deviation`: the API is complete and only registered single-machine topology differences remain. `blocked`: in scope, but implementation or evidence is incomplete; do not claim compatibility. `unsupported`: an explicit non-target product. Target gaps must not be marked `unsupported`. Target members without exact evidence stay `blocked`; do not infer support from a product smoke test or from type presence.

The current inventory has 2,097 target members and no `blocked` item. Workers, KV, R2, D1, Durable Objects, Queues, Cron, Workflows, and Cache API are `supported_with_deviation` because of registered single-machine differences. Alarms, Version Metadata, and WebSocket hibernation are `supported`. D1 covers database/session/prepared-statement/result/meta, errors and bind conversions, atomic batches, opaque bookmarks, and the current hosted non-alpha `dump()` rejection. General raw-TCP outbound uses the one public Network and stock workerd's `cloudflare:sockets`/Node socket implementation; named Service/DO `Fetcher.connect()` uses an explicit capability tunnel. `deployments`, `static_assets`, `service_bindings`, `workers_cache`, and the bounded local Images binding are platform products; Images does not claim the full hosted Cloudflare Images product. Explicit non-target products such as `ai` and `vectorize` are `unsupported`.

## limits

Frozen product-specific numeric ceilings from config. **No secrets.** Exact numbers come from the current `capabilities --json`; do not copy this page or the default TOML as live quotas. The pinned stock OSS workerd standalone `LimitEnforcer` does not enforce Cloudflare's hosted request-scoped CPU, subrequest, or simultaneous-connection quotas; see `OC-WKR-LIMIT-001`, and do not infer those quotas from any other `limits` field.

## Registered deviations

These IDs appear in the matching product `deviations`. Their meaning is the registry; a missing ID does not imply other Cloudflare behavior is available.

### OC-WKR-TCP-001

Tenant general outbound `fetch()`, `cloudflare:sockets.connect()`, and `node:net` share the one stock-workerd `Network(allow = ["public"])`; named Service/DO `Fetcher.connect()` uses a declared capability tunnel rather than a second general outbound path. open-compute does not reproduce Cloudflare-owned IP-range blocking, the Worker self-connect/TCP-loop detector, or the default SMTP port 25 prohibition. Runtime-source, binding-backend, and workerd-internal listeners are loopback-only. Control/data listeners default to loopback but an operator may explicitly expose them, so the public Network does not additionally reject an address based on platform ownership. The operator owns exposed ingress and additional public-network/SMTP egress policy.

### OC-WKR-LIMIT-001

The pinned stock OSS workerd standalone `LimitEnforcer` does not enforce subrequest or CPU limits. open-compute does not claim Cloudflare's request-scoped CPU, subrequest, or simultaneous-connection quotas. This does not relax the public-address security boundary, product-specific limits, handle cleanup, or process supervision.

### OC-KV-001

KV is single-node SQLite authority; it does not claim Cloudflare global replication or propagation timing.

### OC-R2-001

R2 object bytes are held by the configured S3-compatible provider. The platform does not claim Cloudflare global placement or replication.

### OC-D1-001

D1 is a single local-primary SQLite authority. The platform does not claim read-replica or region routing,
hosted `served_by` identity, region/colo metadata, or Cloudflare billing counters. Opaque bookmarks preserve same-database
local sequential visibility; `rows_read` and `rows_written` are stable local SQLite execution counters.

### OC-DO-001

Durable Objects are placed on the single local workerd process. Location hints, jurisdiction, and global migration have no geographic scheduling effect.

### OC-QUEUE-001

Queue producers and push consumers are backed by single-node `scheduler.sqlite` durability, not Cloudflare global replication. Delivery is at-least-once without global FIFO. An unknown native dispatch retains its lease and does not consume the tenant retry budget, so a later delivery can repeat the same attempt number.

### OC-CRON-001

Cron is UTC-only with five fields and the documented local Quartz-like extensions. Recovery projects at most the newest slot within the configured misfire grace rather than replaying complete downtime history. Known failures use the configured bounded local retry policy unless `noRetry()` is called.

### OC-WORKFLOW-001

Workflow execution uses local SQLite authority. Callbacks are at-least-once until their result commits; replay skips durably completed callbacks, and external product effects do not roll back with Workflow snapshots. The platform does not claim cross-region execution, global placement, or Cloudflare dashboard/observability.

### OC-CACHE-001

Workers Cache and Cache API are single-node local authority. Automatic caching requires an explicit `s-maxage` or `max-age`; heuristic TTL, global replication/purge propagation, tiered cache, Cache Rules, Cache Deception Armor, and plan-dependent behavior are unsupported.

### OC-CACHE-002

The operator-configured default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. The exact active values are emitted by `platformd capabilities --json`.

### OC-IMAGES-001

Images is a bounded local raster transform binding, not hosted Cloudflare Images. Hosted delivery/upload/signing, URL transforms, video, AI upscale, and Cloudflare product quotas are out of scope.

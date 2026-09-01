# Compatibility

Trust the binary on this machine. Do not infer full Cloudflare behavior from product names. Cloudflare features that do not appear in `capabilities --json` are unsupported. This page is not a complete Cloudflare matrix.

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. Without it, `limits` come from the embedded default config; with an absolute config path, `limits` reflect that file. Top-level JSON fields: `schema_version`, `release`, `runtime`, `products`, `limits`.

The generated member index (do not hand-copy 2,097 signatures here): [API reference](/en/platform/reference/api/). Registered differences: [Deviations](/en/platform/deviations). Live numeric ceilings: [Limits](/en/platform/limits).

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

Do not swap a workerd beside the binary, search `PATH`, or download another runtime. A digest mismatch is a stop condition. Tenants cannot choose `compatibilityDate` or flags.

## products

Keyed by stable product name. Each entry has:

| Field | Meaning |
| --- | --- |
| `status` | `supported` / `supported_with_deviation` / `blocked` / `unsupported` |
| `kind` | `target` (upstream AST inventory), `platform` (platform-owned), or `non_target` (explicitly out of scope) |
| `capability_version` | Static facade version when the product is fully supported; omitted for `blocked` / `unsupported` |
| `members` | Per-member/overload records for target products. Each record has a stable id, symbol, member, kind, overload, readonly/optional/static, signature, `signature_sha256`, status, and evidence cases |
| `deviations` | Registered single-machine topology or stock-runtime capacity deviation IDs |

## Status semantics

- `supported`: every target member has compile and real-runtime evidence.
- `supported_with_deviation`: the API is complete and only registered single-machine topology differences remain.
- `blocked`: in scope, but implementation or evidence is incomplete; do not claim compatibility.
- `unsupported`: an explicit non-target product. Target gaps must not be marked `unsupported`. Target members without exact evidence stay `blocked`; do not infer support from a product smoke test or from type presence.

The current inventory has 2,097 target members and no `blocked` item. Where signatures match [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/), use that text; this platform's differences are only the registered `OC-*` IDs.

## Products table

| Product | Status | Members | Deviation |
| --- | --- | ---: | --- |
| Workers | `supported_with_deviation` | 1,580 | `OC-WKR-TCP-001`, `OC-WKR-LIMIT-001` |
| KV | `supported_with_deviation` | 52 | `OC-KV-001` |
| R2 | `supported_with_deviation` | 110 | `OC-R2-001` |
| D1 | `supported_with_deviation` | 36 | `OC-D1-001` |
| Durable Objects | `supported_with_deviation` | 115 | `OC-DO-001` (connect members also TCP/limit) |
| Alarms | `supported` | 7 | — |
| Queues | `supported_with_deviation` | 63 | `OC-QUEUE-001` |
| Cron | `supported_with_deviation` | 26 | `OC-CRON-001` |
| Workflows | `supported_with_deviation` | 72 | `OC-WORKFLOW-001` |
| Cache API | `supported_with_deviation` | 14 | `OC-CACHE-001`, `OC-CACHE-002` |
| Version Metadata | `supported` | 3 | — |
| WebSocket hibernation | `supported` | 19 | — |

`deployments`, `static_assets`, `service_bindings`, `workers_cache`, and the bounded local Images binding are platform products (`kind=platform`), with `OC-DEPLOY-001`, `OC-ASSETS-001`, `OC-SERVICE-001`, `OC-CACHE-001` / `OC-CACHE-002`, and `OC-IMAGES-001`. Images does not claim the full hosted Cloudflare Images product.

D1 covers database/session/prepared-statement/result/meta, errors and bind conversions, atomic batches, opaque bookmarks, and the current hosted non-alpha `dump()` rejection. General raw-TCP outbound uses the one public Network and stock workerd's `cloudflare:sockets`/Node socket implementation; named Service/DO `Fetcher.connect()` uses an explicit capability tunnel.

Explicit non-target products: [Unsupported](/en/platform/unsupported).

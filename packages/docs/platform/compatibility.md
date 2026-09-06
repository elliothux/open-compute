# Compatibility

open-compute implements the declared Cloudflare Workers programming model. For each product that is provided, the Worker API matches Cloudflare's documentation. The topology is a single node (one `ocd`, one pinned `workerd`).

Live limits and release identity:

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` is optional for `capabilities`. Without it, `limits` come from the embedded default config; with an absolute config path, `limits` reflect that file.

Worker-side signatures: [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Index on this site: [API reference](/platform/reference/api/). Single-node behavior: [Behavior differences](/platform/deviations). Numeric ceilings: [Limits](/platform/limits).

## Products

| Product | Worker API | Topology difference |
| --- | --- | --- |
| [Workers](/workers/) | Module Workers (`fetch` / `scheduled` / `queue`) | Single-node `workerd`; no global edge |
| [KV](/kv/) | `KVNamespace` | Local SQLite; no global replication |
| [R2](/r2/) | `R2Bucket` | Object bytes on the selected Local or S3 authority; no global placement |
| [D1](/d1/) | `D1Database` | Local SQLite primary; no read replicas or region routing |
| [Durable Objects](/durable-objects/) | `DurableObject` / `DurableObjectNamespace` | Placed on the single local workerd process |
| [Alarms](/durable-objects/alarms) | `state.storage.setAlarm` | Local scheduler |
| [Queues](/queues/) | `Queue` and consumer handlers | Local `scheduler.sqlite`; at-least-once, no global FIFO |
| [Cron](/workers/configuration/cron-triggers) | `scheduled` handler | UTC, five fields; recovery projects at most the newest misfire |
| [Workflows](/workflows/) | Workflows API | Local SQLite; callbacks are at-least-once until commit |
| [Cache API](/workers/runtime-apis/cache) | `caches.default` | Single-node cache |
| [Workers Cache](/workers/cache/) | Automatic HTTP cache | Single node; requires explicit `s-maxage` / `max-age` |
| [Static Assets](/workers/static-assets/) | Assets binding | Immutable Local/S3 content served locally; no global CDN |
| [Service Bindings](/workers/runtime-apis/bindings) | `Fetcher` | Same platform; no cross-region discovery |
| [Deployments](/workers/versions-and-deployments/) | Versions, promotion, rollback | Local SQLite and one runtime generation |
| [Images](/images/) | Images binding | Bounded local raster transforms; not hosted Cloudflare Images |
| [Vectorize](/vectorize/) | Stable post-beta `Vectorize` | Local exact search; per-index SQLite; beta `VectorizeIndex` out of scope |
| [AI Search](/ai-search/) | `env.AI` Markdown Conversion + AI Search | Operator-configured OpenAI-compatible providers; full Workers AI inference not provided |
| [Version Metadata](/workers/runtime-apis/bindings) | Deployment `id` / `tag` / `timestamp` | Produced by local deploy authority |
| [WebSocket hibernation](/workers/runtime-apis/websockets) | Hibernatable WebSockets | On the local Durable Object process |

D1 covers database / session / prepared statement / result / meta, errors and bind conversions, atomic batches, and opaque bookmarks. General TCP outbound uses the one public Network and stock workerd's `cloudflare:sockets` / Node socket implementation; named Service / DO `Fetcher.connect()` uses an explicit capability tunnel.

## Runtime

The platform freezes the compatibility date. `wrangler.jsonc` cannot set `compatibilityDate` or flags. Use `runtime.effective_compatibility_date`. Do not swap a workerd beside the binary, search `PATH`, or download another runtime.

## Management

Management surfaces are separate from product bindings:

| Surface | Status |
| --- | --- |
| Cloudflare v4 API | █████████░ 90% — Local `/client/v4` works with Wrangler and the official SDK. Matching every hosted Cloudflare response still needs a Cloudflare account token. |
| Wrangler | █████████░ 95% — Pinned Wrangler `4.127.1`: deploy and resource commands verified against a running `ocd`. |
| Dashboard | ████████░░ 80% — Operator admin UI on the same `/client/v4` APIs — not a clone of the Cloudflare dashboard. |
| Workers Logs / realtime tail | █████████░ 90% — `wrangler tail` plus Workers Logs query and live tail on a single node. Tail Workers, distributed traces, and Logpush are not provided. |

## Partial / planning / not yet

| Module | Status |
| --- | --- |
| [Vectorize](/vectorize/) | ████████░░ 80% — Partial: stable post-beta `Vectorize` API; beta `VectorizeIndex` out of scope. |
| Markdown Conversion | ████████░░ 80% — Partial: via standard `env.AI` (`toMarkdown`). See [AI Search](/ai-search/). |
| [AI Search](/ai-search/) | ████████░░ 80% — Partial: RAG with operator-configured OpenAI-compatible providers. |
| Browser Run | ██░░░░░░░░ 20% — Planning (formerly Browser Rendering). |
| Artifacts | ██░░░░░░░░ 20% — Planning. |
| Workers AI | ░░░░░░░░░░ 0% — Not yet: hosted model inference is not provided. |
| Containers | ░░░░░░░░░░ 0% — Not yet. |
| Hyperdrive | ░░░░░░░░░░ 0% — Not yet. |
| Analytics Engine | ░░░░░░░░░░ 0% — Not yet. |
| Workers for Platforms | ░░░░░░░░░░ 0% — Not yet. |
| Dynamic Workers | ░░░░░░░░░░ 0% — Not yet. |
| Pipelines | ░░░░░░░░░░ 0% — Not yet. |
| Rate Limiting | ░░░░░░░░░░ 0% — Not yet. |
| mTLS certificates | ░░░░░░░░░░ 0% — Not yet. |
| Tail Workers / traces / Logpush | ░░░░░░░░░░ 0% — Not yet. |

Full list and config field names: [Unsupported](/platform/unsupported). Live surface: `ocd capabilities --json`.

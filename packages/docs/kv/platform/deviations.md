# Behavior differences

The KV Worker API matches Cloudflare. The storage topology does not.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [Cloudflare KV API](https://developers.cloudflare.com/kv/api/) | Same: `put` / `get` / `getWithMetadata` / `list` / `delete`, text / json / arrayBuffer / stream, metadata, TTL, bulk get, list cursor |
| Replication | Global edge | Single-node SQLite on the node running ocd |
| `cacheTtl` | Colo cache | Parameter accepted; no colo cache |
| Jurisdictions | Available | Not provided |
| REST / `client.v4` | Available | Not provided; use the Worker binding |
| REST bulk write / delete | Available | Not provided |

See [Compatibility](/platform/compatibility).

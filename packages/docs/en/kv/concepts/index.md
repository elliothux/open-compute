# Concepts

Each KV namespace is its own SQLite database (`data/kv/<account>/<namespace>/`). Keys sort as UTF-8 bytes with no Unicode normalization. `put` is an atomic replacement in one transaction. A failed write leaves the previous value. Expired keys are immediately absent on read and list.

`cacheTtl` is accepted. There is no colo cache: a read is the current SQLite value.

Keys are at most 512 bytes, and cannot be empty, `.`, or `..`. Values are at most 25 MiB. Serialized metadata is at most 1024 bytes. Minimum `expirationTtl` is 60 seconds. Bulk `get` is at most 100 keys. `list` defaults to / caps at 1000. Types and error shapes match the [KV API](https://developers.cloudflare.com/kv/api/).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [Cloudflare KV API](https://developers.cloudflare.com/kv/api/) | Same: `put` / `get` / `getWithMetadata` / `list` / `delete`, text / json / arrayBuffer / stream, metadata, TTL, bulk get, list cursor |
| Replication | Global edge | Single-node SQLite on the node running ocd |
| `cacheTtl` | Colo cache | Parameter accepted; no colo cache |
| Jurisdictions | Available | Not provided |
| REST bulk write / delete | Available | Not provided |

Next: [Guides](/en/kv/guides/).

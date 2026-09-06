# Behavior differences

Vectorize on open-compute is a single-node exact index, not Cloudflare's globally distributed Vectorize service.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | Stable post-beta `Vectorize` | Same public methods |
| Search | Managed approximate topology | Deterministic exact search |
| Storage | Hosted Vectorize | Per-index local SQLite; Local/S3 as needed for platform object backends |
| Beta `VectorizeIndex` | Legacy | Not provided |
| Global placement / replication | Available | Not provided |
| Fleet-scale quotas | Hosted plans | Local create-time quotas |

See [Compatibility](/platform/compatibility) and [Cloudflare Vectorize](https://developers.cloudflare.com/vectorize/).
